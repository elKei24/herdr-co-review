## [1.1.1](https://github.com/elKei24/herdr-co-review/compare/v1.1.0...v1.1.1) (2026-08-12)


### Bug Fixes

* correct MSRV to 1.88 and enforce it in CI ([1640d4c](https://github.com/elKei24/herdr-co-review/commit/1640d4cbbebf943f4f805a85ce72cceebd1d14a4))

# [1.1.0](https://github.com/elKei24/herdr-co-review/compare/v1.0.0...v1.1.0) (2026-08-12)


### Bug Fixes

* satisfy clippy::question_mark in parse_github_remote ([135aaf4](https://github.com/elKei24/herdr-co-review/commit/135aaf45c77af079b520a7e00dc00c7c2ccc5aef))


### Features

* add curl|sh installer for prebuilt binaries ([a76149f](https://github.com/elKei24/herdr-co-review/commit/a76149f4d250154fe475b1a772608890079f3277))

# 1.0.0 (2026-08-12)


### Bug Fixes

* correctness fixes from code review ([839e0c5](https://github.com/elKei24/herdr-co-review/commit/839e0c5673374054be67e8fff8804c558cd5f2f3))
* restore terminal on TUI panic ([9c62ce4](https://github.com/elKei24/herdr-co-review/commit/9c62ce45eda02dc921413a212847d726e871073e))
* second code-review pass ([530cc60](https://github.com/elKei24/herdr-co-review/commit/530cc6048b45a52c9842f15d5569e7b2e3c1ca0c))
* third code-review pass ([198f703](https://github.com/elKei24/herdr-co-review/commit/198f7034434fc8ef37771220fbce983ccb24ee0a))


### Features

* add `co-review edit` to revise a finding ([6700085](https://github.com/elKei24/herdr-co-review/commit/6700085034461c2dca6eaa32649caecb29b40c4c))
* CLI, agent/human commands, diff viewer, and orchestrator ([721c521](https://github.com/elKei24/herdr-co-review/commit/721c521fbf5a6f4057afab3ac5a68b6b216b15a1))
* fall back to a PR comment when an inline comment is rejected ([4d2cc99](https://github.com/elKei24/herdr-co-review/commit/4d2cc99ad665f0946c095136cacf06ce577fcd2e))
* findings navigator TUI (ratatui + syntect) ([8b4de27](https://github.com/elKei24/herdr-co-review/commit/8b4de2718af32f3627006362d538698e7decb817))
* git, GitHub, and Herdr integration layers ([f0a2c6c](https://github.com/elKei24/herdr-co-review/commit/f0a2c6cde96c2c335e7a0c871c91d4547852506d))
* graceful fallback when Herdr automation fails ([1d3b023](https://github.com/elKei24/herdr-co-review/commit/1d3b0235a73addc7256c50bb499de06bf5731302))
* reopen the navigator by PR reference (`co-review view 123`) ([e758cd0](https://github.com/elKei24/herdr-co-review/commit/e758cd07c4f77c73de52741ef1bf7e40d85c9c7a))
* robust herdr workspace-id fallback; verify resume in tests ([78e50ad](https://github.com/elKei24/herdr-co-review/commit/78e50adde4b0cf06514788f0d19ee980f4bc768d))
* session lifecycle commands (sessions, end) ([ed813e9](https://github.com/elKei24/herdr-co-review/commit/ed813e986fe67274ea0caef28e6ff5ad2b859738))
* session state model and lock-guarded store ([431ad9f](https://github.com/elKei24/herdr-co-review/commit/431ad9f9beaaa1f60081792a209ff05ff4d8c0bb))
* ship prebuilt binaries; plugin installs without Rust ([3a5cd7c](https://github.com/elKei24/herdr-co-review/commit/3a5cd7c2b098b57d2c50b82725629d5fd93c4515))
