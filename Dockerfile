FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p agent-hub
# distroless has no shell, so make the data dir here and hand it to nonroot.
RUN mkdir -p /data

FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/agent-hub /agent-hub
COPY --from=build --chown=65532:65532 /data /data
ENV HUB_BIND=0.0.0.0:8787 HUB_DB=/data/hub.db
VOLUME /data
EXPOSE 8787
USER nonroot
ENTRYPOINT ["/agent-hub"]
