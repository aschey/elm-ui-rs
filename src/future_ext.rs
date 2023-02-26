// Largely taken from https://github.com/Finomnis/tokio-graceful-shutdown/blob/ec444f69e884d27a48bef7ad88abe91b9ab7a648/src/future_ext.rs
use futures::Future;
use pin_project_lite::pin_project;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

#[derive(thiserror::Error, Debug)]
#[error("A shutdown request caused this task to be cancelled")]
pub struct CancelledByShutdown;

pin_project! {
    #[must_use = "futures do nothing unless polled"]
    pub struct CancelOnShutdownFuture<'a, T: Future>{
        #[pin]
        future: T,
        #[pin]
        cancellation: WaitForCancellationFuture<'a>,
    }
}

impl<T: Future> Future for CancelOnShutdownFuture<'_, T> {
    type Output = Result<T::Output, CancelledByShutdown>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        match this.cancellation.as_mut().poll(cx) {
            Poll::Ready(()) => return Poll::Ready(Err(CancelledByShutdown)),
            Poll::Pending => (),
        }

        match this.future.as_mut().poll(cx) {
            Poll::Ready(res) => Poll::Ready(Ok(res)),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub trait FutureExt {
    type Future: Future;

    fn cancel_on_shutdown(
        self,
        cancellation_token: &CancellationToken,
    ) -> CancelOnShutdownFuture<'_, Self::Future>;
}

impl<T: Future> FutureExt for T {
    type Future = T;

    fn cancel_on_shutdown(
        self,
        cancellation_token: &CancellationToken,
    ) -> CancelOnShutdownFuture<'_, T> {
        let cancellation = cancellation_token.cancelled();
        CancelOnShutdownFuture {
            future: self,
            cancellation,
        }
    }
}
