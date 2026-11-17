//! Who is asking: the MCP client a request comes from, as it introduced
//! itself (its name and version), and which connection of it.
//!
//! A tool call runs with its caller known (`scope`), and the editor call
//! it makes on a thread of its own (`on_thread`), where [`caller`] gives
//! it: so that what an agent changes is credited to that agent, whatever
//! the agent says of itself in the call.

use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;

/// An MCP client.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Caller {
    pub name: String,
    pub version: String,
    /// One connection of it, as long as the server runs.
    pub session: String,
}

tokio::task_local! {
    static CALLER: Option<Caller>;
}

thread_local! {
    static ON_THREAD: RefCell<Option<Caller>> = const { RefCell::new(None) };
}

/// The client whose request the current thread is answering, if any.
#[must_use]
pub fn caller() -> Option<Caller> {
    ON_THREAD.with(|caller| caller.borrow().clone())
}

/// The client at the other end of `peer`.
pub(crate) fn of(peer: &rmcp::Peer<rmcp::RoleServer>) -> Option<Caller> {
    let info = peer.peer_info()?;
    let started = std::process::id();
    Some(Caller {
        name: info.client_info.name.clone(),
        version: info.client_info.version.clone(),
        session: format!("{started:x}-{:x}", Arc::as_ptr(&info) as usize),
    })
}

/// Runs `future` with `caller` as the caller.
pub(crate) async fn scope<F: Future>(caller: Option<Caller>, future: F) -> F::Output {
    CALLER.scope(caller, future).await
}

/// The caller of the tool call being run, to take to another thread.
pub(crate) fn current() -> Option<Caller> {
    CALLER.try_with(Clone::clone).ok().flatten()
}

/// Runs `call` with `caller` as the caller of this thread.
pub(crate) fn on_thread<T>(caller: Option<Caller>, call: impl FnOnce() -> T) -> T {
    let before = ON_THREAD.with(|known| known.replace(caller));
    let done = call();
    ON_THREAD.with(|known| known.replace(before));
    done
}
