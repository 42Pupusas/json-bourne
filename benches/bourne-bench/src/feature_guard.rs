// `alloc-profile` installs divan's AllocProfiler as the process-global
// allocator; `compare-mem`'s `compare_mem` binary installs its own counting
// allocator. Two global allocators cannot link in one build, so enabling
// both features together is a configuration error rather than a
// missing-symbol surprise at link time.
#[cfg(all(feature = "alloc-profile", feature = "compare-mem"))]
compile_error!(
    "features `alloc-profile` and `compare-mem` are mutually exclusive: \
     each installs a #[global_allocator]"
);
