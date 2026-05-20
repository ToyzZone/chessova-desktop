use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
};
use tracing::{debug, warn};

use crate::protocol::{EngineLine, HelperMessage, EngineId};

pub struct Engine {
    engine_id: EngineId,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Engine {
    /// Spawn a UCI engine at `path`, send `uci`, apply any UCI `setoption`
    /// pairs from `options`, then `isready` + wait for `readyok`.
    pub async fn spawn(
        path: &str,
        engine_id: EngineId,
        options: &[(String, String)],
    ) -> Result<Self> {
        let mut child = Command::new(path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn engine at {}", path))?;

        let stdin = child.stdin.take().context("no stdin on engine process")?;
        let stdout = child.stdout.take().context("no stdout on engine process")?;
        let stdout = BufReader::new(stdout);

        let mut engine = Self {
            engine_id,
            child,
            stdin,
            stdout,
        };

        engine.write_line("uci").await?;
        for (name, value) in options {
            engine
                .write_line(&format!("setoption name {} value {}", name, value))
                .await?;
        }
        engine.write_line("isready").await?;
        engine.wait_for("readyok").await?;

        Ok(engine)
    }

    /// Run `position fen <fen>` + `go depth <depth> multipv <multipv>`.
    /// Streams `analyze-progress` messages; sends `analyze-done` on `bestmove`.
    /// If `stop_rx` is provided and fires, sends UCI `stop` to the engine and
    /// continues draining stdout until the engine reports `bestmove`.
    pub async fn analyze(
        &mut self,
        request_id: &str,
        fen: &str,
        multi_pv: u32,
        depth: u32,
        stream: bool,
        tx: mpsc::Sender<HelperMessage>,
        stop_rx: Option<oneshot::Receiver<()>>,
    ) -> Result<()> {
        // Borrow stdin/stdout as separate locals so tokio::select! can hold a
        // read_line future on stdout while the stop branch writes to stdin.
        let Self {
            engine_id,
            stdin,
            stdout,
            ..
        } = self;

        write_uci(stdin, &format!("setoption name MultiPV value {}", multi_pv)).await?;
        write_uci(stdin, &format!("position fen {}", fen)).await?;
        write_uci(stdin, &format!("go depth {}", depth)).await?;

        let mut best_lines: Vec<EngineLine> = Vec::new();
        let mut last_depth: u32 = 0;
        let mut stop_rx = stop_rx;
        let mut stopped = false;

        let mut line_buf = String::new();
        loop {
            line_buf.clear();

            let read_n: usize;
            if let Some(rx) = stop_rx.as_mut() {
                tokio::select! {
                    biased;
                    res = stdout.read_line(&mut line_buf) => {
                        read_n = res?;
                    }
                    _ = rx => {
                        // Stop requested — write UCI stop, then keep reading
                        // until bestmove so the engine flushes cleanly.
                        let _ = write_uci(stdin, "stop").await;
                        stopped = true;
                        stop_rx = None;
                        continue;
                    }
                }
            } else {
                read_n = stdout.read_line(&mut line_buf).await?;
            }

            if read_n == 0 {
                break; // EOF
            }
            let line = line_buf.trim();
            debug!(engine = ?engine_id, raw = %line, "uci output");

            if line.starts_with("bestmove") {
                if !stopped {
                    let _ = tx
                        .send(HelperMessage::AnalyzeDone {
                            id: request_id.to_string(),
                            engine: engine_id.clone(),
                            fen: fen.to_string(),
                            depth: last_depth,
                            lines: best_lines,
                        })
                        .await;
                }
                break;
            }

            if line.starts_with("info") {
                if let Some(info) = parse_info_line(line) {
                    let pv_depth = info.depth;
                    let idx = info.multipv.saturating_sub(1) as usize;
                    let engine_line: EngineLine = info.into();
                    if idx < best_lines.len() {
                        best_lines[idx] = engine_line;
                    } else {
                        best_lines.push(engine_line);
                    }
                    last_depth = pv_depth;

                    if stream && !stopped {
                        let _ = tx
                            .send(HelperMessage::AnalyzeProgress {
                                id: request_id.to_string(),
                                engine: engine_id.clone(),
                                fen: fen.to_string(),
                                depth: pv_depth,
                                lines: best_lines.clone(),
                            })
                            .await;
                    }
                }
            }
        }

        Ok(())
    }

    /// Send `stop` to the engine process.
    pub async fn stop(&mut self) {
        if let Err(e) = self.write_line("stop").await {
            warn!(engine = ?self.engine_id, err = %e, "failed to send stop");
        }
    }

    async fn write_line(&mut self, cmd: &str) -> Result<()> {
        write_uci(&mut self.stdin, cmd).await
    }

    async fn wait_for(&mut self, token: &str) -> Result<()> {
        let mut buf = String::new();
        loop {
            buf.clear();
            self.stdout.read_line(&mut buf).await?;
            if buf.trim() == token {
                return Ok(());
            }
        }
    }
}

async fn write_uci(stdin: &mut ChildStdin, cmd: &str) -> Result<()> {
    stdin
        .write_all(format!("{}\n", cmd).as_bytes())
        .await
        .context("write to engine stdin")?;
    stdin.flush().await.context("flush engine stdin")?;
    Ok(())
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

// ---------------------------------------------------------------------------
// UCI info line parser
// ---------------------------------------------------------------------------

struct InfoLine {
    depth: u32,
    multipv: u32,
    eval_cp: Option<i32>,
    eval_mate: Option<i32>,
    pv: Vec<String>,
}

impl From<InfoLine> for EngineLine {
    fn from(il: InfoLine) -> Self {
        Self {
            pv: il.pv,
            eval_cp: il.eval_cp,
            eval_mate: il.eval_mate,
            depth: il.depth,
        }
    }
}

/// Parse a UCI `info depth N multipv K score cp X pv u1 u2 ...` line.
/// Returns None if the line has no `pv` token (e.g. `info string` lines).
fn parse_info_line(line: &str) -> Option<InfoLine> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut depth: u32 = 0;
    let mut multipv: u32 = 1;
    let mut eval_cp: Option<i32> = None;
    let mut eval_mate: Option<i32> = None;
    let mut pv: Vec<String> = Vec::new();
    let mut in_pv = false;

    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "depth" => {
                i += 1;
                depth = tokens.get(i).and_then(|t| t.parse().ok()).unwrap_or(0);
            }
            "multipv" => {
                i += 1;
                multipv = tokens.get(i).and_then(|t| t.parse().ok()).unwrap_or(1);
            }
            "score" => {
                i += 1;
                match tokens.get(i) {
                    Some(&"cp") => {
                        i += 1;
                        eval_cp = tokens.get(i).and_then(|t| t.parse().ok());
                    }
                    Some(&"mate") => {
                        i += 1;
                        eval_mate = tokens.get(i).and_then(|t| t.parse().ok());
                    }
                    _ => {}
                }
            }
            "pv" => {
                in_pv = true;
            }
            token if in_pv => {
                pv.push(token.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    if pv.is_empty() {
        return None;
    }

    Some(InfoLine {
        depth,
        multipv,
        eval_cp,
        eval_mate,
        pv,
    })
}
