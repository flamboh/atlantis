# syntax=docker/dockerfile:1

FROM ubuntu:24.04 AS build-base

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        autoconf \
        automake \
        bison \
        build-essential \
        ca-certificates \
        cmake \
        curl \
        flex \
        git \
        libbz2-dev \
        libtool \
        pkg-config \
        python3 \
        tar \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

FROM build-base AS nfdump-builder

ARG NFDUMP_COMMIT

# Clone the exact superproject gitlink commit. A copied submodule has a .git
# pointer into the host repository, so it cannot support compile-nfdump.sh.
RUN test -n "$NFDUMP_COMMIT" \
    || { echo "NFDUMP_COMMIT must name the vendor/nfdump gitlink commit" >&2; exit 1; }
RUN git clone https://github.com/flamboh/nfdump.git vendor/nfdump \
    && git -C vendor/nfdump checkout --detach "$NFDUMP_COMMIT" \
    && test "$(git -C vendor/nfdump rev-parse HEAD)" = "$NFDUMP_COMMIT"

COPY vendor/scripts/compile-nfdump.sh vendor/scripts/compile-nfdump.sh
RUN ./vendor/scripts/compile-nfdump.sh

FROM build-base AS rust-builder

ENV PATH="/root/.cargo/bin:${PATH}"

COPY rust-toolchain.toml rust-toolchain.toml
RUN curl --proto '=https' --tlsv1.2 --silent --show-error --fail \
        https://sh.rustup.rs \
        | sh -s -- --yes --profile minimal --default-toolchain none \
    && cargo --version

COPY Cargo.toml Cargo.lock ./
COPY tools/netflow-db/Cargo.toml tools/netflow-db/Cargo.toml
COPY tools/netflow-db/src tools/netflow-db/src
RUN cargo build --locked --release --package atlantis-netflow-db --bin netflow-db

FROM ubuntu:24.04 AS runtime

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libbz2-1.0 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /build/target/release/netflow-db /usr/local/bin/netflow-db
COPY --from=nfdump-builder /build/target/nfdump/libexec/nfdump /usr/local/bin/nfdump

RUN --mount=type=bind,from=nfdump-builder,source=/build/target/nfdump/build/smoke/dummy_flows.nf,target=/tmp/dummy_flows.nf \
    netflow-db contract-version >/dev/null \
    && nfdump -V >/dev/null 2>&1 \
    && nfdump -G none -r /tmp/dummy_flows.nf -q -o atlantis 'host 203.0.113.255' >/tmp/atlantis.bin \
    && test "$(wc -c </tmp/atlantis.bin)" -eq 16 \
    && rm /tmp/atlantis.bin

WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/netflow-db"]
