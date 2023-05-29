use std::{
    env, error, fmt, fs,
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    time,
};

use serde::{Deserialize, Serialize};

use crate::{
    gpu::CYCLES_FULL_REFRESH,
    sched::{EventType, FutureEvent, Scheduler},
};
use rustboyadvance_utils::Shared;

#[derive(Debug)]
enum TxSocketState {
    Open(UnixStream),
    Retrying(String), // error string
}

pub(crate) struct SocketIO {
    tx: TxSocketState,
    tx_fn: PathBuf,
    rx: UnixListener,
    // print errors out every Nth error to prevent spam
    errcnt: usize,
    i: usize,
}

// we notify once every 30 event refreshes
const ERR_COUNT_NOTIFY: usize = 30;

// when modulus of i and v is 0, we send
const SEND_FREQ: usize = 50;

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct GameState<'a, 'b> {
    pub(crate) iwram: &'a [u8],
    pub(crate) ewram: &'b [u8],
    pub(crate) time: usize,
}

impl SocketIO {
    pub(crate) fn new(sched: Option<Shared<Scheduler>>) -> Self {
        let tx_fn =
            &env::var("GBA_TX_SOCKET_NAME").unwrap_or_else(|_| "/tmp/gba_tx.sock".to_string());
        let rx_fn =
            &env::var("GBA_RX_SOCKET_NAME").unwrap_or_else(|_| "/tmp/gba_rx.sock".to_string());
        let tx_fn = Path::new(tx_fn);
        let rx_fn = Path::new(rx_fn);

        if rx_fn.exists() {
            fs::remove_file(&rx_fn).unwrap();
        }
        let rx = UnixListener::bind(&rx_fn).unwrap();
        println!("now listening for IO on {}", rx_fn.to_string_lossy());

        if let Some(mut sched) = sched {
            sched.schedule_at(EventType::SocketIO, CYCLES_FULL_REFRESH * 5);
        }

        let mut sock_io = SocketIO {
            tx: TxSocketState::Retrying("first run".to_string()),
            tx_fn: tx_fn.to_path_buf(),
            rx,
            errcnt: 0,
            i: 0,
        };

        let _ = sock_io.try_open_tx();
        println!("SocketIO initialized");

        sock_io
    }

    pub(crate) fn on_event(&mut self, state: GameState) -> Option<FutureEvent> {
        self.i += 1;
        let can_err = self.errcnt % ERR_COUNT_NOTIFY == 0;

        if self.try_open_tx().is_err() {
            self.errcnt += 1;
        }

        match &mut self.tx {
            TxSocketState::Retrying(err) if can_err => {
                println!(
                    "failed to open tx socket {}: {}",
                    self.tx_fn.to_string_lossy(),
                    err
                );
            }
            TxSocketState::Open(sock) if self.i % SEND_FREQ == 0 => {
                match bincode::serialize(&state) {
                    Err(e) => println!("failed serialize state: {}", e),
                    Ok(s) => {
                        if let Err(e) = sock.write_all(&s) {
                            self.errcnt += 1;
                            if can_err {
                                println!("failed to write state to socket: {}", e);
                                self.tx = TxSocketState::Retrying(e.to_string());
                            }
                        } else {
                            self.errcnt = 0;
                        }
                    }
                };
            }
            _ => (),
        }

        Some((EventType::SocketIO, CYCLES_FULL_REFRESH))
    }

    fn try_open_tx(&mut self) -> Result<(), &String> {
        use TxSocketState::*;

        match self.tx {
            Open(_) => Ok(()),
            Retrying(_) => {
                self.tx = match UnixStream::connect(&self.tx_fn) {
                    Ok(ln) => {
                        ln.set_nonblocking(true).unwrap();
                        ln.set_write_timeout(Some(time::Duration::from_micros(100)))
                            .unwrap();
                        TxSocketState::Open(ln)
                    }
                    Err(e) => TxSocketState::Retrying(e.to_string()),
                };

                match &self.tx {
                    Open(_) => Ok(()),
                    Retrying(err) => Err(&err),
                }
            }
        }
    }
}
