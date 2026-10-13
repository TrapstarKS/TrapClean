# Decision providers and privacy

AI is optional and disabled by default. Scanning, local recommendations, duplicate verification, and reviewed cleanup work offline. A provider recommendation cannot change local eligibility, select items, authorize cleanup, or execute commands.

## Configuration

| Mode | Endpoint | Initial model | Key |
| --- | --- | --- | --- |
| Disabled | No requests | Not used | Not needed |
| OpenAI | `https://api.openai.com/v1/decisions` | `gpt-6-luna` | Required |
| JEV / TypeSafe | `https://api.typesafe.ai/v1/systemone` | `jev-latest` | Required |
| Custom | Complete URL entered by the user | Required model/alias | Optional Bearer key |

OpenAI uses the dedicated Decisions API. Only `gpt-6-luna` was documented for that API when checked on 2026-10-13, so this version validates that model. It does not substitute Chat Completions or Responses. See the [official Decisions guide](https://developers.openai.com/api/docs/guides/decisions).

The TypeSafe adapter uses its own contract: `state`, a named `questions` object, and `criteria`. Analysis requests `choice`; the synthetic connection test requests `noul`, whose number estimates the probability of yes/true. Returned answers are a map keyed by the same question names. `jev-latest` is the alias shown in the current OpenAPI examples; account-specific availability is not verified. Available models can be checked through the service's `GET /v1/models`. See the [official TypeSafe OpenAPI](https://api.typesafe.ai/openapi.json) and [interactive documentation](https://api.typesafe.ai/docs).

Custom posts to the exact configured endpoint and requires the [textual Decisions contract](custom-provider.md). HTTPS is required except for loopback HTTP. Redirects are disabled; TLS verification remains enabled. All provider requests connect directly: automatic system and environment proxies are disabled. `localhost` is pinned to the physical IPv4/IPv6 loopback addresses (`127.0.0.1` and `::1`), preserving the configured URL, Host header, and port. Networks requiring a proxy cannot use providers in this version. Keys are scoped to the provider, and a Custom key is scoped to its endpoint, so changing the Custom URL does not send a previously saved key to a new service.

Test connection runs only when the user clicks its button. It sends a fixed synthetic question, never scan metadata. Analyze with AI runs only for explicitly selected candidates after the UI describes the data being sent. Both actions may consume provider credits. Startup, scanning, and opening Settings do not send provider requests. Paid requests are not automatically retried.

## Keys

The Rust backend stores keys with the native macOS Keychain or Windows Credential Manager through `keyring`. Keys are never returned to the frontend, stored in SQLite, included in scan exports, or printed in provider errors. The settings response contains only a missing/saved/session status. The entered key travels once from the settings form to the backend; the form then clears it.

If the system credential vault cannot save the key, saving fails with an explicit message. The user may select **session-only** storage: that key stays in backend memory until the application closes. There is no plaintext fallback. Remove key deletes the active provider's credential and clears its in-memory replacement and decision cache. Session-only replacement does not delete an older saved vault credential; remove that saved key first if it should no longer exist.

The connection test is the way to check a saved credential and endpoint. The app makes no claims about provider retention policies or account permissions; consult the selected provider's terms.

## Metadata leaving the device

An analysis request contains at most 20 logical candidates. It sends these fields:

- Indexed candidate IDs and root-relative paths with arbitrary personal/project/file names replaced by `[redacted]`. Only a fixed set of tool output components, such as `target`, `node_modules`, `debug`, `.cache`, and `.next`, is retained. Redaction is declared in the payload.
- File/directory kind, local category, logical and allocated bytes, scan coverage, local recommendation, and local eligibility.
- Last modified and reported filesystem access timestamps, or null when unknown. **Usage is always marked unknown**; a timestamp is not “last used.”
- Whether a project was detected and a fixed set of observed rule facts, such as Cargo manifest presence, preserved sources, dependency lockfile presence, recognized build/cache layout, protected data, or incomplete coverage. Dependency availability and local modifications remain unverified.

It does not send the root path, usernames, arbitrary filenames, file contents, complete manifests, freeform on-disk text, errors containing paths, keys, or the entire tree. All disk-derived names remain untrusted, and returned decisions must reference only requested IDs. Local templates supply explanations and consequences; the provider does not generate them.

Requests are limited to 32 KiB; responses to 256 KiB. Candidate path size and depth are bounded. Requests above the limit are rejected before transmission, with no silent truncation. Network requests use a 5-second connection timeout and 15-second overall timeout. Cancellation is checked before sending and about every 100 ms while waiting. It cannot recall metadata already received by a provider.

## Results, cache, and failures

Results are Candidate for cleanup, Review required, or Keep. “Insufficient evidence” is Review required. The probability of the selected class and provider confidence remain distinct. Neither proves that deleting data is safe. Missing numerical fields are not invented. Refusals have no probability or confidence.

SQLite stores nonsecret preferences and at most 500 decision records, retained for up to seven days. Each fingerprint includes provider, endpoint, model, policy version, and all candidate facts. Changes invalidate that candidate; changing settings or removing a key clears the cache. Cache entries contain only IDs, normalized recommendations, and provider numbers, never request metadata or keys. If any candidate is missing from cache, the selected batch is requested together. Refused or invalid responses are not cached.

Authentication errors, rate limits, redirects, unavailable providers, timeout, cancellation, invalid JSON, and incompatible schemas produce explicit status. Affected candidates return Review required and local evidence remains available. Errors never enable more aggressive cleanup.

## Verification

Unit and contract tests use real localhost HTTP connections with synthetic temporary fixtures. They cover request shapes, redaction and instruction-like names, IDs, response distributions, refusals, authentication, rate limits, malformed JSON/schema, redirects, timeout, cancellation, response bounds, missing keys, cache invalidation/size, and direct localhost requests despite proxy settings. The proxy regression runs in a bounded child process so other tests' environment stays unchanged. Tests use in-memory key handling and never touch a user's existing vault credential or call a paid provider.

A passing mock test demonstrates the application's contract and error handling. Real provider/account access and native credential-vault behavior require separate runtime verification; they are not established by mock tests.
