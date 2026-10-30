# GPU allocation probe

`gpu_test` is copied unchanged from runpod/runpod-python commit
`ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5`, under the Runpod MIT license
in `LICENSE-UPSTREAM`. The upstream binary targets Linux x86_64, glibc 2.31+, and
CUDA 11.8+ drivers. Its source/build instructions live in the upstream
`build_tools/compile_gpu_test.sh` and `docs/serverless/gpu_binary_compilation.md`.

The Rust SDK embeds this resource only for Linux x86_64 and extracts it into an
owned executable temporary file when configuring GPU checks. An existing
`RUNPOD_BINARY_GPU_TEST_PATH` takes precedence. Other platforms use an explicitly
provided native executable or nvidia-smi fallback. No Python interpreter is used.
