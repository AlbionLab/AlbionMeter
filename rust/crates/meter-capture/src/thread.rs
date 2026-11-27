use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};

use crate::{AdapterStatistics, CaptureError, CaptureFrame, FrameSource, Read};

const STATISTICS_EVERY: Duration = Duration::from_millis(200);

/// Why the capture thread stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// `cancel()` (or dropping the handle): the player stopped.
    Stopped,
    /// The source ended by itself; `CaptureService` keeps the record paused and reopens the device later.
    SourceEnded,
    Failed(CaptureError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureExit {
    pub reason: ExitReason,
    pub statistics: Option<AdapterStatistics>,
    pub counters: CaptureCounters,
}

/// What the thread handed over and what it had to drop because the engine fell behind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureCounters {
    pub sent: u64,
    pub channel_dropped: u64,
}

#[derive(Default)]
struct Shared {
    sent: AtomicU64,
    channel_dropped: AtomicU64,
    has_statistics: AtomicBool,
    received: AtomicU32,
    dropped: AtomicU32,
    interface_dropped: AtomicU32,
}

impl Shared {
    fn store(&self, statistics: Option<AdapterStatistics>) {
        if let Some(statistics) = statistics {
            self.received.store(statistics.received, Ordering::Relaxed);
            self.dropped.store(statistics.dropped, Ordering::Relaxed);
            self.interface_dropped
                .store(statistics.interface_dropped, Ordering::Relaxed);
        }
        self.has_statistics
            .store(statistics.is_some(), Ordering::Release);
    }

    fn counters(&self) -> CaptureCounters {
        CaptureCounters {
            sent: self.sent.load(Ordering::Relaxed),
            channel_dropped: self.channel_dropped.load(Ordering::Relaxed),
        }
    }
}

/// A source read on its own thread into a bounded channel. When the engine falls behind, new frames are dropped and
/// counted instead of queueing without bound; the engine hands spent `data` buffers back through `recycler()`.
pub struct CaptureThread {
    frames: Receiver<CaptureFrame>,
    recycle: Sender<Vec<u8>>,
    cancel: Arc<AtomicBool>,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<CaptureExit>>,
}

impl CaptureThread {
    pub fn spawn(source: Box<dyn FrameSource>, capacity: usize) -> std::io::Result<Self> {
        let (sender, frames) = crossbeam_channel::bounded(capacity);
        let (recycle, spare) = crossbeam_channel::bounded(capacity);
        let cancel = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::default());
        let handle = std::thread::Builder::new().name("capture".into()).spawn({
            let (cancel, shared) = (cancel.clone(), shared.clone());
            move || run(source, &sender, &spare, &cancel, &shared)
        })?;
        Ok(Self {
            frames,
            recycle,
            cancel,
            shared,
            handle: Some(handle),
        })
    }

    pub fn frames(&self) -> &Receiver<CaptureFrame> {
        &self.frames
    }

    /// Send a frame's `data` back here once it is decoded, so the next packet reuses it.
    pub fn recycler(&self) -> Sender<Vec<u8>> {
        self.recycle.clone()
    }

    /// Stops the thread at its next poll timeout (≤ 100 ms).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn counters(&self) -> CaptureCounters {
        self.shared.counters()
    }

    /// The source's own counters, refreshed every 200 ms.
    pub fn statistics(&self) -> Option<AdapterStatistics> {
        self.shared
            .has_statistics
            .load(Ordering::Acquire)
            .then(|| AdapterStatistics {
                received: self.shared.received.load(Ordering::Relaxed),
                dropped: self.shared.dropped.load(Ordering::Relaxed),
                interface_dropped: self.shared.interface_dropped.load(Ordering::Relaxed),
            })
    }

    /// Cancels and waits for the thread.
    pub fn join(self) -> CaptureExit {
        self.cancel();
        self.wait()
    }

    /// Waits for the thread without cancelling it (the source ended or failed).
    pub fn wait(mut self) -> CaptureExit {
        let shared = self.shared.clone();
        self.handle
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_else(|| CaptureExit {
                reason: ExitReason::Failed(CaptureError::Io(
                    "The capture thread stopped unexpectedly.".into(),
                )),
                statistics: None,
                counters: shared.counters(),
            })
    }
}

impl Drop for CaptureThread {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run(
    mut source: Box<dyn FrameSource>,
    sender: &Sender<CaptureFrame>,
    spare: &Receiver<Vec<u8>>,
    cancel: &AtomicBool,
    shared: &Shared,
) -> CaptureExit {
    let link_type = source.link_type();
    let fresh = |spare: &Receiver<Vec<u8>>| CaptureFrame {
        link_type,
        data: spare.try_recv().unwrap_or_default(),
        ..CaptureFrame::default()
    };
    let mut frame = fresh(spare);
    let mut next_statistics = Instant::now();
    let reason = loop {
        if cancel.load(Ordering::Relaxed) {
            break ExitReason::Stopped;
        }
        match source.read(&mut frame) {
            // Single producer: a channel that is not full now stays so until this send.
            Ok(Read::Frame) if sender.is_full() => {
                _ = shared.channel_dropped.fetch_add(1, Ordering::Relaxed)
            }
            Ok(Read::Frame) => match sender.try_send(std::mem::replace(&mut frame, fresh(spare))) {
                Ok(()) => _ = shared.sent.fetch_add(1, Ordering::Relaxed),
                Err(TrySendError::Full(_)) => {
                    _ = shared.channel_dropped.fetch_add(1, Ordering::Relaxed)
                }
                // The engine is gone: nobody will read another frame.
                Err(TrySendError::Disconnected(_)) => break ExitReason::Stopped,
            },
            Ok(Read::Idle) => {}
            Ok(Read::Ended) => break ExitReason::SourceEnded,
            Err(error) => break ExitReason::Failed(error),
        }
        if Instant::now() >= next_statistics {
            shared.store(source.statistics());
            next_statistics = Instant::now() + STATISTICS_EVERY;
        }
    };
    let statistics = source.statistics();
    shared.store(statistics);
    CaptureExit {
        reason,
        statistics,
        counters: shared.counters(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Burst {
        remaining: usize,
        idle_forever: bool,
        reads: Arc<AtomicUsize>,
    }

    impl FrameSource for Burst {
        fn link_type(&self) -> i32 {
            crate::LINK_TYPE_RAW
        }
        fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.remaining == 0 {
                if self.idle_forever {
                    std::thread::sleep(Duration::from_millis(5));
                    return Ok(Read::Idle);
                }
                return Ok(Read::Ended);
            }
            self.remaining -= 1;
            frame.timestamp_us = self.remaining as i64;
            frame.original_length = 3;
            frame.data.clear();
            frame.data.extend_from_slice(&[1, 2, 3]);
            Ok(Read::Frame)
        }
        fn statistics(&mut self) -> Option<AdapterStatistics> {
            Some(AdapterStatistics {
                received: 7,
                dropped: 1,
                interface_dropped: 0,
            })
        }
    }

    fn burst(remaining: usize, idle_forever: bool) -> (Box<dyn FrameSource>, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        (
            Box::new(Burst {
                remaining,
                idle_forever,
                reads: reads.clone(),
            }),
            reads,
        )
    }

    #[test]
    fn full_channel_drops_and_counts_instead_of_growing() {
        let (source, _) = burst(100, false);
        let capture = CaptureThread::spawn(source, 4).unwrap();
        let frames = capture.frames().clone();
        let exit = capture.wait();
        assert_eq!(exit.reason, ExitReason::SourceEnded);
        assert_eq!(
            exit.counters,
            CaptureCounters {
                sent: 4,
                channel_dropped: 96
            }
        );
        assert_eq!(
            exit.statistics,
            Some(AdapterStatistics {
                received: 7,
                dropped: 1,
                interface_dropped: 0
            })
        );
        let kept: Vec<_> = frames.try_iter().collect();
        assert_eq!(kept.len(), 4);
        // The first frames are kept, later ones dropped.
        assert_eq!(kept[0].timestamp_us, 99);
        assert!(
            kept.iter()
                .all(|frame| frame.data == [1, 2, 3] && frame.link_type == crate::LINK_TYPE_RAW)
        );
    }

    #[test]
    fn consumer_that_keeps_up_loses_nothing() {
        let (source, _) = burst(1000, false);
        let capture = CaptureThread::spawn(source, 8).unwrap();
        let received = capture.frames().iter().count();
        let exit = capture.wait();
        assert_eq!(received as u64 + exit.counters.channel_dropped, 1000);
        assert_eq!(exit.counters.sent, received as u64);
    }

    #[test]
    fn recycled_buffers_are_reused() {
        struct Gated(Arc<AtomicUsize>, usize);
        impl FrameSource for Gated {
            fn link_type(&self) -> i32 {
                1
            }
            fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
                if self.1 >= self.0.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                    return Ok(Read::Idle);
                }
                self.1 += 1;
                frame.data.clear();
                frame.data.push(self.1 as u8);
                Ok(Read::Frame)
            }
            fn statistics(&mut self) -> Option<AdapterStatistics> {
                None
            }
        }
        let allowed = Arc::new(AtomicUsize::new(1));
        let capture = CaptureThread::spawn(Box::new(Gated(allowed.clone(), 0)), 4).unwrap();
        let next = || {
            capture
                .frames()
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
        };
        let first = next();
        // The thread already holds a buffer for the second packet; the recycled one serves the third.
        capture.recycler().send(Vec::with_capacity(9999)).unwrap();
        drop(first);
        allowed.store(2, Ordering::Release);
        assert_eq!(next().data, [2]);
        allowed.store(3, Ordering::Release);
        let third = next();
        assert_eq!(third.data, [3]);
        assert!(third.data.capacity() >= 9999);
        assert_eq!(capture.join().reason, ExitReason::Stopped);
    }

    #[test]
    fn cancel_stops_the_thread_promptly() {
        let (source, reads) = burst(0, true);
        let capture = CaptureThread::spawn(source, 4).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(!capture.is_finished());
        assert!(reads.load(Ordering::Relaxed) > 0);
        let started = Instant::now();
        let exit = capture.join();
        assert_eq!(exit.reason, ExitReason::Stopped);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn source_errors_end_the_thread() {
        struct Failing;
        impl FrameSource for Failing {
            fn link_type(&self) -> i32 {
                1
            }
            fn read(&mut self, _: &mut CaptureFrame) -> Result<Read, CaptureError> {
                Err(CaptureError::Io(
                    "The capture socket became unavailable.".into(),
                ))
            }
            fn statistics(&mut self) -> Option<AdapterStatistics> {
                None
            }
        }
        let exit = CaptureThread::spawn(Box::new(Failing), 1).unwrap().wait();
        assert_eq!(
            exit.reason,
            ExitReason::Failed(CaptureError::Io(
                "The capture socket became unavailable.".into()
            ))
        );
        assert_eq!(exit.statistics, None);
    }
}
