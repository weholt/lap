mod atomic_write;
mod common;
mod corrupt_and_readonly;
mod projection_reconcile;
mod revision_cas;

#[test]
fn crash_worker_entrypoint() {
    if let Ok(spec) = std::env::var(common::CRASH_SPEC_ENV) {
        common::run_crash_worker(&spec);
    }
}
