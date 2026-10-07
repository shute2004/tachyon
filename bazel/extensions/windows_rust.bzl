load("@rules_rust//rust/private:repositories.bzl", "rust_repository_set")

def _windows_gnullvm_rust_impl(module_ctx):
    rust_repository_set(
        name = "wr",
        versions = ["1.95.0"],
        edition = "2024",
        exec_triple = "x86_64-pc-windows-gnullvm",
        exec_compatible_with = [
            "@platforms//cpu:x86_64",
            "@platforms//os:windows",
            "@llvm//constraints/windows/abi:gnullvm",
            "@llvm//constraints/windows/crt:msvcrt",
        ],
        default_target_compatible_with = [
            "@platforms//cpu:x86_64",
            "@platforms//os:windows",
            "@llvm//constraints/windows/abi:gnullvm",
            "@llvm//constraints/windows/crt:msvcrt",
        ],
        aliases = {
            "wr__x86_64-pc-windows-gnullvm__stable": "wr_gnullvm_stable",
        },
    )
    return module_ctx.extension_metadata(reproducible = True)

windows_gnullvm_rust = module_extension(
    doc = "Defines the stable native Windows gnullvm Rust toolchain.",
    implementation = _windows_gnullvm_rust_impl,
)
