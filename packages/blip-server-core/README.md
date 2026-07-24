# Blip Server Core

Runtime-independent Effect v4 server functionality shared by the Node and
Cloudflare applications.

It contains:

- The typed `HttpApi` and OpenAPI contract.
- User authentication and per-user S3 configuration.
- AES-GCM credential encryption.
- Multipart upload and private playback domain logic.
- SQL repository services shared by SQLite and D1.

Runtime packages provide an Effect SQL client, HTTP platform, configuration, and
process lifecycle. This package has no Alchemy, Cloudflare, Node HTTP, D1, or
SQLite dependency.
