//! The call on the main thread, with a thread that stands for the main
//! thread and its queue.

use super::*;
use std::sync::mpsc::Sender;
use std::thread;

/// A thread named "main-queue" that runs each job it gets, as the main
/// dispatch queue does.
fn main_queue() -> Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    thread::Builder::new()
        .name("main-queue".into())
        .spawn(move || {
            for job in rx {
                job();
            }
        })
        .unwrap();
    tx
}

fn thread_name() -> Option<String> {
    thread::current().name().map(str::to_string)
}

#[test]
fn a_call_from_another_thread_runs_on_the_main_queue() {
    let q = main_queue();
    let caller = thread::Builder::new()
        .name("computer-conn".into())
        .spawn(move || {
            // The thread-check hook: the call records the thread it runs on.
            run(
                false,
                |job| q.send(job).unwrap(),
                thread_name,
                Duration::from_secs(5),
            )
        })
        .unwrap();
    let ran_on = caller.join().unwrap();
    assert_eq!(ran_on, Some(Some("main-queue".into())));
}

#[test]
fn a_call_on_the_main_thread_runs_here() {
    let here = thread_name();
    let r = run(
        true,
        |_| panic!("no post from the main thread: it would wait for itself"),
        thread_name,
        Duration::from_secs(5),
    );
    assert_eq!(r, Some(here));
}

#[test]
fn a_busy_main_thread_gives_none_after_the_wait() {
    // The main queue never runs the job.
    let (tx, _rx) = mpsc::channel::<Job>();
    let r = run(
        false,
        |job| tx.send(job).unwrap(),
        || 'x',
        Duration::from_millis(20),
    );
    assert_eq!(r, None);
}
