use std::{
    env, error, fmt, fs,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
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
}

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

        if tx_fn.exists() {
            fs::remove_file(&tx_fn).unwrap();
        }
        let rx = UnixListener::bind(&rx_fn).unwrap();
        println!("now listening for IO on {}", tx_fn.to_string_lossy());

        if let Some(mut sched) = sched {
            sched.schedule_at(EventType::SocketIO, CYCLES_FULL_REFRESH * 5);
        }

        let tx = match UnixStream::connect(&tx_fn) {
            Ok(ln) => TxSocketState::Open(ln),
            Err(e) => {
                let err = e.to_string();
                println!("failed to open tx socket: {}", &err);
                TxSocketState::Retrying(err)
            }
        };

        println!("INITIALIZED");
        SocketIO {
            tx,
            tx_fn: tx_fn.to_path_buf(),
            rx,
            errcnt: 0,
        }
    }

    pub(crate) fn on_event(&mut self, state: GameState) -> Option<FutureEvent> {
        println!("EVENT GET: {}", state.time);
        Some((EventType::SocketIO, CYCLES_FULL_REFRESH))
    }

    fn try_open_tx(&mut self) -> Result<(), &String> {
        use TxSocketState::*;

        match self.tx {
            Open(_) => Ok(()),
            Retrying(_) => {
                self.tx = match UnixStream::connect(&self.tx_fn) {
                    Ok(ln) => TxSocketState::Open(ln),
                    Err(e) => {
                        let err = e.to_string();
                        println!("failed to open tx socket: {}", &err);
                        TxSocketState::Retrying(err)
                    }
                };

                match &self.tx {
                    Open(_) => Ok(()),
                    Retrying(err) => Err(&err),
                }
            }
        }
    }
}
