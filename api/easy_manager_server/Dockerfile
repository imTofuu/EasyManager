FROM lukemathwalker/cargo-chef:0.1.75-rust-1.93.1-trixie AS chef
WORKDIR /app

FROM chef AS planner

COPY . .

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .

RUN cargo build --release

FROM debian:trixie-slim AS runtime
WORKDIR /app

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* 

COPY --from=builder /app/target/release/easy_manager_server easy_manager_server

EXPOSE 3000
ENTRYPOINT ["/app/easy_manager_server"]