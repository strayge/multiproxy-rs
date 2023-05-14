macro_rules! create_socket_coroutines {
    ($name:expr, $read_func:expr, $write_func:expr, $close_actions:expr) => {
        let token = CancellationToken::new();
        let token2 = token.clone();

        tokio::spawn(async move {
            tokio::select! {
                res = $read_func => {
                    if res.is_err() {
                        error!("read_{} error: {}", $name, res.err().unwrap());
                    }
                    else {
                        info!("read_{} end", $name);
                    }
                    token.cancel();
                }
                _ = token.cancelled() => {
                    error!("read_{} cancelled", $name);
                }
            }
        });

        tokio::spawn(async move {
            tokio::select! {
                res = $write_func => {
                    if res.is_err() {
                        error!("write_{} error: {}", $name, res.err().unwrap());
                    }
                    else {
                        info!("write_{} end", $name);
                    }
                    token2.cancel();
                }
                _ = token2.cancelled() => {
                    error!("write_{} cancelled", $name);
                }
            }
            $close_actions;
        });
    };
}

pub(crate) use create_socket_coroutines;
