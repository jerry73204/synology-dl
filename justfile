default:
    @just --list

build:
    cargo build --profile dev-release

install:
    cargo install --path .

format:
    cargo fmt

check:
    cargo fmt --check
    cargo clippy --profile dev-release -- -D warnings

clean:
    cargo clean
