use std::{
    future::Future,
    pin::Pin,
};
use tide::{
    middleware::Next,
    Context, Response,
};

pub fn handle<'a, State: 'static + Send + Sync>(cx: Context<State>, next: Next<'a, State>) -> Pin<Box<(dyn Future<Output = Response> + Send + 'a)>> {
    Box::pin(async {
        let span = tracing::span!(tracing::Level::INFO, "client request handler", method = ?cx.method(), path = ?cx.uri());
        let _guard = span.enter();
        next.run(cx).await
    })
}
