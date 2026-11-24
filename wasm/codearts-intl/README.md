# CodeArts International

Independent provider `codearts-intl`; does not reuse or migrate domestic accounts.
Requires the gateway CodeArts regional host support on repair branch `fix/codearts-intl-20261124`.

The implementation shares the domestic protocol code with the `intl` build feature.
Regional endpoints come from **HuaweiCloud.vscode-codebot 26.10.300**, public `extension/product.json`,
`commercialVersionDomain.HKFramework.productDomain`:

- Portal: `https://codearts.ap-southeast-1.huaweicloud.com/portal`
- Inference, catalog, usage and delivery: `https://snap-access.ap-southeast-1.myhuaweicloud.com`
- OAuth exchange and renewal: `https://sts.cn-north-4.myhuaweicloud.com`

The STS service is shared by the official client profiles. It must not be changed to the Hong Kong region.
The domestic legacy developer benefit-model gateway is not declared in the official HK profile;
this package queries regional Agent/builtin catalogs instead. Welfare availability and amounts are
reported by the regional delivery API; no benefit amount is assumed or fabricated.

Login uses the international Huawei Cloud portal. Complete login personally and select the desired
proxy in the gateway. Credential imports must come from the international account/session.
Secrets and account identities are scoped to this provider; heartbeat uses the selected account's
frozen egress for the entire conversation.

Build and verification run in the personal GitHub repair branch workflow. No local Rust build is needed.

## Package-aware welfare

The official 26.10.300 `getWelfareDelivery` checks subscription
`package.is_token_package === true` and returns no credit campaigns without
requesting delivery. The international Free / `codearts.agent.trial` account
was verified read-only to have this flag; it must not be sent to credit check-in.
The package flags are retained in the usage projection. Token allowances and
remaining usage use token units, rather than an unknown credit balance.
Unknown package types and authentication failures remain distinct from this
explicitly inapplicable case. No successful check-in or grant is recorded for it.
