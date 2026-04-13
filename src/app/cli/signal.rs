use std::sync::atomic::{AtomicBool, Ordering};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
const SIGINT: i32 = 2;

pub(crate) fn install_handler() {
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    install_handler_impl();
}

pub(crate) fn is_stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}

#[cfg(unix)]
fn install_handler_impl() {
    type SignalHandler = unsafe extern "C" fn(i32);

    unsafe extern "C" fn handle_sigint(_: i32) {
        STOP_REQUESTED.store(true, Ordering::SeqCst);
    }

    unsafe extern "C" {
        fn signal(signum: i32, handler: SignalHandler) -> SignalHandler;
    }

    unsafe {
        signal(SIGINT, handle_sigint);
    }
}

#[cfg(not(unix))]
fn install_handler_impl() {}
