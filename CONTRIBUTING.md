# Contributing

## Development setup

Install a stable Rust toolchain and the `pre-commit` framework. This repository pins the required Rust components in `rust-toolchain.toml`.

```sh
uv tool install pre-commit
pre-commit install --install-hooks
```

Other supported ways to install `pre-commit` are documented at <https://pre-commit.com/>.

Run the complete local quality suite with:

```sh
pre-commit run --all-files
cargo test --all-features --locked
```

The installed hooks run:

- Prettier 3.9.6 for Markdown files
- `cargo fmt` and Clippy before commits
- tests before pushes
- general whitespace, YAML, TOML, conflict, large-file, and private-key checks
- Conventional Commit validation for commit messages

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/):

```text
feat(editor): add command reordering
fix(runner): stop waiting on retained output pipes
docs: explain cargo-binstall installation
```

The accepted default types are `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `style`, and `test`.

## Pull requests

Before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --locked
npx --yes prettier@3.9.6 --check "**/*.md"
```

CI repeats these checks and verifies that the crate can be packaged.

## Releases

1. Update the version in `Cargo.toml`.
2. Commit using a Conventional Commit message.
3. Push a tag matching the Cargo version, such as `v0.2.0`.

The release workflow verifies the version, publishes the crate, creates a GitHub release, and uploads checksummed binaries for Linux, macOS, and Windows. These assets follow the naming convention declared in the cargo-binstall metadata.

Publishing requires a crates.io API token in the repository's `CARGO_REGISTRY_TOKEN` Actions secret. After the first release, the project can migrate to [crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing) and remove the long-lived token.
