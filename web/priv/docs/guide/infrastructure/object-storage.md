# Object Storage Cache

An object storage bucket can hold Struo's shared cache. Every machine keeps its
local cache, consults the bucket on a local miss, and publishes new results to
it. No cache server runs between them: Struo talks to the storage service
directly.

Use it for development teams and CI fleets that already have a bucket, for a
self-hosted cache, or to try shared caching without an account anywhere.

## Configure A Bucket

Name the bucket in the repository root `struo.toml`:

```toml
[infrastructures.team_cache]
kind = "object_store"
url = "s3://acme-build-cache/struo"

[infrastructure.cache]
provider = "team_cache"
```

The URL scheme selects the service, and the path after the bucket is a prefix
Struo writes under:

| URL | Service |
| --- | --- |
| `s3://bucket/prefix` | Amazon S3 and S3-compatible services |
| `gs://bucket/prefix` | Google Cloud Storage |
| `az://container/prefix` | Azure Blob Storage |
| `file:///absolute/directory` | A local or mounted directory |

Keep credentials out of the URL. Struo rejects passwords, queries, and
fragments, and permits a user-name component only for Azure's
`abfss://container@account.dfs.core.windows.net/prefix` container syntax
and its `az://` and `abfs://` equivalents.

`options` passes service settings, such as an endpoint or region:

```toml
[infrastructures.team_cache]
kind = "object_store"
url = "s3://acme-build-cache/struo"
options = { endpoint = "https://ACCOUNT_ID.r2.cloudflarestorage.com", region = "auto" }
```

Option names follow the configuration keys of the Rust `object_store` library,
with or without their `aws_`, `google_`, or `azure_` prefix, plus `profile` for
S3 (see [Authenticate](#authenticate)). Unknown names are rejected. Credential
options, such as `secret_access_key` or `account_key`, are also rejected so
that secrets never land in a configuration file; set them in the environment
instead. Options that only point at credentials, such as a token file path or a
metadata endpoint URI, are allowed.

The same `kind = "object_store"` table works in `~/.config/struo/config.toml`
for a personal default, and as `[cache_provider]` with `kind = "object_store"`
for a single workspace.

## Authenticate

Struo reads credentials from the environment variables and ambient identity
each service's own tools use:

- **Amazon S3 and compatible services:** the AWS SDK's default credential
  chain, the same one the AWS CLI uses. It reads, in order:
  `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN`; the
  profile named by `AWS_PROFILE` (or `default`) in `~/.aws/config` and
  `~/.aws/credentials`, including IAM Identity Center (SSO) sessions,
  `credential_process` helpers, and roles assumed from a source profile; web
  identity through `AWS_WEB_IDENTITY_TOKEN_FILE` and `AWS_ROLE_ARN`, which
  covers GitHub Actions and other CI OpenID Connect federation; ECS and EKS
  container credentials; and the EC2 instance metadata service. Temporary
  credentials are renewed five minutes before they expire.
- **Google Cloud Storage:** `GOOGLE_SERVICE_ACCOUNT` or
  `GOOGLE_APPLICATION_CREDENTIALS` naming a key file, the application default
  credentials that `gcloud auth application-default login` writes, and the
  metadata server on Google Cloud.
- **Azure Blob Storage:** `AZURE_STORAGE_ACCOUNT_NAME` with
  `AZURE_STORAGE_ACCOUNT_KEY`, a SAS token in `AZURE_STORAGE_SAS_TOKEN`, a
  service principal through `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, and
  `AZURE_TENANT_ID`, workload identity through `AZURE_FEDERATED_TOKEN_FILE`,
  managed identity, or the Azure CLI when `use_azure_cli = "true"`.

To pin a profile for a provider instead of relying on `AWS_PROFILE`, set it
in `options`:

```toml
[infrastructures.team_cache]
kind = "object_store"
url = "s3://acme-build-cache/struo"
options = { profile = "build-cache" }
```

When neither `options` nor `AWS_REGION` sets a region, Struo uses the
profile's `region`. For an SSO profile, sign in with `aws sso login --profile
<name>` first; Struo reuses the cached session but does not open a browser. An
expired session is reported like an outage: actions run locally and a warning
says why.

Setting `skip_signature`, `role_arn`, `web_identity_token_file`, or a
container or metadata endpoint in `options` switches S3 back to the storage
library's own credential lookup, which does not read `~/.aws` profiles; such
options cannot be combined with `profile`.

## Restrict Untrusted Jobs

Content is verified against its digest before Struo uses it, but an action
result is a record whose correctness Struo cannot check. Anyone who can write
to the bucket can publish a result that other machines reuse. Give untrusted
jobs, such as pull requests from forks, read-only credentials, and make Struo
stop publishing for them:

```toml
[infrastructures.team_cache]
kind = "object_store"
url = "s3://acme-build-cache/struo"
read_only = true
```

Setting `STRUO_CACHE_READ_ONLY=1` has the same effect for one invocation on the
object storage cache. It does not disable Tuist uploads. The environment can
only make access narrower; it cannot turn publishing back on for a provider
configured with `read_only = true`.

## Manage Retention

Struo never deletes objects from the bucket. Local garbage collection and
`struo cache action forget` affect only the local cache. Configure retention
in the storage service, for example a lifecycle rule that expires objects
under the prefix after 30 days and aborts incomplete multipart uploads after
one day.

Expiring content is safe. A shared result whose referenced content has been
deleted is treated as a miss, and Struo runs the action again.

## Failures

- **Unreachable or throttled service:** each request retries up to three times
  within 30 seconds. After that, the action runs locally and the result is kept
  in the local cache.
- **Missing or rejected credentials, or denied access:** the command fails
  rather than silently running without the shared cache. This includes an S3
  profile that does not exist or no credentials anywhere in the AWS chain. Fix
  the credentials, or set `STRUO_CACHE_PROVIDER=local` to bypass the bucket for
  one invocation. An explicit STS authentication or role-permission refusal
  also fails the command rather than falling back to local execution, as do
  rejected Azure or Google OAuth token exchanges.
- **Unreachable credential source:** when the source exists but cannot answer,
  such as the SSO portal or STS while offline, or when an SSO session has
  expired, the action runs locally. The warning gives safe recovery guidance;
  credential helper output, including Azure CLI stderr, and raw provider
  diagnostic details are not logged because they can contain secrets.
- **Corrupt or truncated content:** it is rejected before it enters the local
  cache, and the command reports the corruption.

Publishing is best effort. A failed upload is logged as a warning, and the
command still succeeds with its local result. Cancelling an action does not
release an upload's memory, staging space, or transfer slot before the
backend finishes the admitted upload.

Reads issue a metadata request before downloading an object so Struo can
reserve memory and staging disk before starting the body transfer. These
requests count toward the storage service's request charges and rate limits.

## Supported Services

The `s3://`, `gs://`, and `az://` backends use each service's REST API, so any
service implementing that API with standard request signing is expected to
work. Struo's own checks have exercised these setups:

| Setup | Status |
| --- | --- |
| S3 API through the moto emulator | Tested: remote reuse across two clients and read-only mode |
| S3 credentials from `~/.aws` profiles, through the moto emulator | Tested: a static-key profile and a `credential_process` profile, selected by `AWS_PROFILE` and by `options.profile`; an unknown profile fails the command |
| S3 credentials from IAM Identity Center (SSO), assumed roles, web identity, ECS, EKS, and EC2 instance metadata | Supported through the AWS SDK credential chain; not yet tested |
| S3 endpoint that is not running | Tested: actions fall back to local execution |
| In-memory store in automated tests | Tested: eviction, corrupt and truncated content, refused reads, upload ordering, staging budget |
| Azure Blob Storage through the Azurite emulator | Tested: remote reuse, multipart upload of a 12 MB blob, refused credentials |
| `file://` directory | Tested, including multipart writes |
| Amazon S3 | Protocol compatible; not yet tested against the live service |
| Google Cloud Storage | Protocol compatible; not yet tested against the live service or an emulator |
| Azure Blob Storage | Protocol compatible; tested only through Azurite |
| Cloudflare R2, MinIO, Backblaze B2, Wasabi, DigitalOcean Spaces, Tigris, Ceph RGW, and other S3-compatible services | Protocol compatible through `endpoint`; not yet tested |

Some S3-compatible services need `virtual_hosted_style_request = "true"` or a
specific `region`, such as `auto` for Cloudflare R2. Plain HTTP endpoints, such
as a local MinIO, need `allow_http = "true"`.

## Limitations

- Content is stored uncompressed, so transfer and storage costs match the raw
  size of outputs.
- Remote downloads check object sizes before fetching their bodies. Each
  remote result lookup also makes one existence check
  per referenced blob not already held locally. Request-priced services charge
  for each.
- A standalone stream larger than 8 MiB, such as a large log written outside an
  action result, stays local until an action result that references it is
  published.
- Isolation is whatever the bucket's access policy provides. Use a separate
  bucket or prefix with its own credentials for each trust boundary.
- A `file://` store on a network filesystem is only as safe for concurrent
  writers as that filesystem's rename.
