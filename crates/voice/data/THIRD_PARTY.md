# Third-party sources of `dictionary.v1.json.gz`

The packaged technical dictionary (design 153 §5.1; survey `docs/design/153-datasets.md`) is composed by
`crates/voice/build-dictionary/build.py` from the sources below. Only names, aliases and spoken forms are taken —
no descriptions, no documentation text. Every source is permissive or attribution-only; share-alike (Wikipedia,
Stack Overflow), non-commercial (Kaggle TN corpus) and GPL/AGPL/LGPL/MPL app tables are deliberately excluded.
Per-source term counts and the sha256 of the composed file are in `dictionary.json`.

| Source | What we take | Licence | URL |
|---|---|---|---|
| talonhub/community | symbols with spoken forms (`core/keys/symbols.py`), casing formatters (`core/formatters/code_formatter.talon-list`), abbreviations (`core/abbreviate/abbreviate.py`), number words (`core/numbers/numbers.py`), file extensions (`core/file_extension/file_extension.py`), TLDs (`core/top_level_domain/top_level_domains.talon-list`) | MIT — Copyright (c) 2019–2026 talonhub/community contributors; https://github.com/talonhub/community/blob/main/LICENSE | https://github.com/talonhub/community |
| Homebrew formulae.brew.sh API | formula `name`, `aliases`, `oldnames`; cask `name`, `token` | BSD-2-Clause — Copyright (c) 2009–present Homebrew contributors; https://github.com/Homebrew/formulae.brew.sh/blob/master/LICENSE.txt | https://formulae.brew.sh/api/formula.json, https://formulae.brew.sh/api/cask.json |
| tldr-pages | page names under `pages/common/` (names only, no page text) | CC BY 4.0 (pages) — Copyright the tldr-pages contributors; https://github.com/tldr-pages/tldr/blob/main/LICENSE.md | https://github.com/tldr-pages/tldr/releases (tldr-pages.en.zip) |
| oh-my-zsh | plugin directory names | MIT — Copyright (c) 2009–2026 Robby Russell and contributors; https://github.com/ohmyzsh/ohmyzsh/blob/master/LICENSE.txt | https://github.com/ohmyzsh/ohmyzsh/tree/master/plugins |
| GitHub linguist | language names and `aliases` from `lib/linguist/languages.yml` | MIT — Copyright (c) 2017 GitHub, Inc.; https://github.com/github-linguist/linguist/blob/main/LICENSE | https://github.com/github-linguist/linguist |
| hugovk/top-pypi-packages | the 15 000 most-downloaded PyPI project names | no licence file; bare package names are facts. Data derived from the public PyPI download statistics (Google BigQuery). | https://hugovk.dev/top-pypi-packages/ |
| botocore | AWS service ids (`botocore/data/<id>/`) and `serviceFullName` / `serviceAbbreviation` from `service-2.json` metadata | Apache-2.0 — Copyright Amazon.com, Inc. or its affiliates; https://github.com/boto/botocore/blob/develop/LICENSE.txt and NOTICE | https://pypi.org/project/botocore/ (sdist) |
| Kubernetes | API `kind` names from `api/openapi-spec/swagger.json` (`x-kubernetes-group-version-kind`) | Apache-2.0 — Copyright The Kubernetes Authors; https://github.com/kubernetes/kubernetes/blob/master/LICENSE | https://github.com/kubernetes/kubernetes |
| models.dev | provider and model ids and display names from `api.json` | MIT — Copyright (c) 2025 Anomaly Innovations; https://github.com/anomalyco/models.dev/blob/dev/LICENSE | https://models.dev/api.json |
| Hugging Face Hub | the top 2 000 model ids by downloads (org and model name) | names only, from the public Hub API; Hub terms https://huggingface.co/terms-of-service | https://huggingface.co/api/models?sort=downloads |
| theuth (this repository) | the hand-curated list of coding agents, harnesses, models, runtimes and tools in `build.py` `CURATED` | ours | — |

Matcher libraries (Rust crates, not data): `rphonetic` (Double Metaphone, Apache-2.0), `strsim` (Levenshtein, MIT).

Not used (see `153-datasets.md` §7): Terraform registry (terms not checked), crates.io dump (terms not readable), CMUdict
(not needed — spoken forms are derived by normalisation, not by pronunciation), Wikipedia, Stack Overflow, the Kaggle TN
corpus, FluidVoice / VoiceInk / Whispering / Caster / dragonfly tables, Jargon File, Microsoft Terminology.
