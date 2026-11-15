# Contributing to Ace Term

Thank you for your interest in Ace Term.

## Branching Model

- `main`: Stable baseline. Only tested milestone releases and hotfixes are merged here.
- `develop`: Active integration branch.
- `feat/<topic>`, `fix/<topic>`, `chore/<topic>`: Working branches created from `develop` and merged back via pull request or fast-forward merge after checks pass.

## Commit Conventions

Use clear, concise commit messages following Conventional Commits:

- `feat(crate): description`
- `fix(crate): description`
- `chore: description`
- `docs: description`

## Required Checks

Before opening a pull request or merging into `develop` or `main`, ensure the following pass cleanly:

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo deny check licenses
```

## Licensing

By contributing to Ace Term, you agree that your contributions will be dual-licensed under the MIT License and the Apache License, Version 2.0. Do not introduce dependencies with GPL, AGPL, or incompatible copyleft licenses.
