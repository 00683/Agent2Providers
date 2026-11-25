# HungerG campaign catalog compatibility

During the official international personal-account promotion (2026-11-24
00:00 through 2026-12-01 00:00, UTC+8), add HungerG when the account model
catalog omits it. Use the reference model key `HungerG`, OpenAI format,
zero-credit label, and low/medium/high/max reasoning levels. Preserve official
entries, including explicit disabled status, pricing, and context metadata.

The same overlay runs for list publication and account-bound request resolution.
Account route caches continue to store the unmodified parsed upstream catalog.
Published supplements carry an expiry marker so a saved public entry is removed
at the next catalog rebuild after the deadline. Request resolution rechecks the
clock on every request. A real upstream entry is retained after the promotion;
the supplement never advertises permanent free access.

The catalog supplement does not grant account entitlement. Upstream membership,
fair-use limits, queueing and errors remain authoritative. Credentials, request
identity, proxies, retry rules and other provider plugins are unchanged.

References:

- https://docs.qoder.com/events/hungerg-free
- https://github.com/shuishuipingan/qoder2api-hub/issues/172
- https://github.com/shuishuipingan/qoder2api-hub/blob/3cc8617e88efd57c76a20a9ca20fb6f224daa913/qoder_catalog_intl.json

Only catalog facts and the campaign-merging behavior are adapted; no Python
implementation or inference transport code is copied. Rust validation and
Wasm packaging run in GitHub Actions, not on the user's machine.
