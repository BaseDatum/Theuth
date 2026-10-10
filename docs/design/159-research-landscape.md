# Autoresearch beyond one metric in one repo, October 2026

Survey of 10 Oct 2026. Category counts come from [awesome-autoresearch](https://github.com/yibie/awesome-autoresearch) on that day. Results are the authors' own claims from READMEs, abstracts and blog titles; none were re-measured. Web search was partly rate-limited, and one survey article ([Autoresearch in the Wild](https://medium.com/@tsepa.stas/autoresearch-in-the-wild-a-survey-of-real-world-applications-181aedf15404)) sat behind a Cloudflare wall and was not read. The earlier evidence behind design 140 (Karpathy, pi-autoresearch, SkyPilot, AIRA, METR) is in the knowledge base and is not repeated here.

## 1. Where it is used

| Category (awesome-autoresearch) | Entries | What gets measured |
|---|---:|---|
| Scientific research | 114 | validation loss or accuracy, benchmark leaderboards, simulator reward |
| Software / systems optimisation | 70 | latency, throughput, build and test time, solver scores |
| Finance / trading | 38 | backtest return, forecast error, credit-model metrics |
| Evaluation / red teaming | 30 | eval pass rate, attack success |
| Marketing & GTM | 5 | holdout error, conversion, retention |
| Workflow automation | 4 | programmatic checks or rubric scores |
| Knowledge base / RAG preparation | 3 | retrieval score |
| Personal knowledge / humanities | 2 | evidence-backed coverage |
| Content research | 1 | source-anchored claims |
| Infra / skills / forks | 169 | tooling to run the loop |
| Related practices / discussions | 180 | write-ups |

The labels have long tails. **Science:** 92 distinct domain labels over 114 entries. Vision, tabular ML, RL, game AI, robotics, medical imaging, drug discovery and materials lead; one-offs include LiDAR SLAM, MRI sequence design, analog circuits, photonic design, quantum experiments, Earth-system models, macro forecasting and sports analytics. **Software:** 64 labels over 70: LLM inference and serving, Apple-silicon inference, search ranking, SAT and heuristic solvers, a ZK prover, chip design, GPU kernels, test speed and reliability, browser automation. **Finance:** crypto (7, plus live, DEX and Bitcoin-forecasting entries), prediction markets (5), equities, options, credit scoring, underwriting and risk (3), sports and value betting.

## 2. Applications with reported results

| Domain | Project | What the agent changes | Metric | Reported result |
|---|---|---|---|---|
| Template engine | [Shopify Liquid](https://simonwillison.net/2026/Mar/13/liquid/) (Tobi Lütke) | engine code | parse + render time | −53 %, 93 automated commits |
| Search ranking service | [idealo](https://medium.com/idealo-tech-blog/one-hour-37-faster-applying-autoresearch-to-our-search-ranking-inference-endpoint-34cffc08e373) | production learning-to-rank inference endpoint | latency | 37 % faster in one hour; preprocessing 5.9× faster |
| ETL | [SQLite ingestion](https://www.luiscolunga.com/posts/autoresearch-etl-optimization/) | Python financial-data ingestion (pi-autoresearch) | ingest time | 588× faster |
| Test reliability | [Gumroad](https://gumroad.com/blog/p/how-i-used-autoresearch-to-fix-gumroad-s-flaky-tests-in-a-week) | flaky tests (OpenClaw autoresearch) | CI stability | 206 commits, 94 CI cycles, one week |
| LLM agent | [Datadog DBM](https://www.datadoghq.com/blog/llm-experimentation-autoresearch/) | SQL-optimisation agent: model size, prompts, a two-pass call | accuracy on an eval set | 59 % more accurate |
| Agent skill | [Langfuse](https://langfuse.com/blog/2026-03-24-optimizing-ai-skill-with-autoresearch) | prompt-migration skill | score on six fixed codebase cases | the loop exposed weak tests |
| Marketing attribution | [mmm-research](https://github.com/lucianfialho/mmm-research) | marketing-mix model | holdout WAPE | 2.73 % → 0.23 % over ~137 experiments (Google Meridian: 4.04 %) |
| ML research | [Paper Lantern](https://www.paperlantern.ai/blog/auto-research-case-study) | training code, with a 2M-paper MCP server | validation loss | 3.2 % lower; 100 papers cited over 100 experiments |
| Lab scaffold | [MiniMax M2.7](https://www.minimax.io/news/minimax-m27-en) | internal research-agent scaffold | internal evals | +30 % over 100+ keep/revert rounds |
| Trading (backtest) | [Lightwater Dispatch](https://lightwaterdispatch.com/dispatch/ai-autoresearch-trading-strategy-backtesting/) | SMA-crossover strategy | backtest total return | 133.8 % → 290.6 % in 30 iterations (see §4) |
| Live trading | [delu-agent](https://github.com/deluagent/delu-agent) | scoring model of a crypto treasury on Base | backtests | 5 parallel loops, 9,000+ experiments |

Notable, without headline numbers:

- **GPU kernels:** [AutoKernel](https://github.com/RightNow-AI/autokernel) ([paper](https://arxiv.org/abs/2603.21331)).
- **LiDAR SLAM on KITTI:** [autoslam](https://github.com/pglira/autoslam).
- **Robotics:** in MuJoCo ([autoresearch-robotics](https://github.com/jellyheadandrew/autoresearch-robotics)), in simulation ([SimEX](https://arxiv.org/html/2609.38982)) and on real robots ([ENPIRE](https://research.nvidia.com/labs/gear/enpire/)).
- **Physical design:** analog circuits with ngspice ([autocircuit](https://github.com/qelloman/autocircuit)); 3D-print infill with FEA ([pla-fea-autoresearch](https://github.com/DJLougen/pla-fea-autoresearch)).
- **Quantum and biology:** quantum experiments screened on cheap noisy simulations ([autoresearch-quantum](https://github.com/saymrwulf/autoresearch-quantum)); the Arc Virtual Cell Challenge ([ram_poc](https://github.com/yashraj59/ram_poc)).
- **Systems:** SAT solvers ([agent-sat](https://github.com/iliazintchenko/agent-sat)); inference serving under throughput, latency and memory limits ([autoresearch-inference-optimization](https://github.com/lawjarp-simplismart/autoresearch-inference-optimization)); RAG pipelines ([AutoRAGsearch](https://github.com/Smasko7/AutoRAGsearch)).
- **Red teaming:** new LLM attack algorithms ([Claudini](https://github.com/romovpa/claudini)); voice-agent prompts tested against adversarial callers ([autovoiceevals](https://github.com/ArchishmanSengupta/autovoiceevals)).
- **Business metrics read through APIs:** cold-email replies, landing-page conversion, ad CTR, newsletter opens, YouTube thumbnail CTR ([autoresearch-anything](https://github.com/grandamenium/autoresearch-anything)).
- **Workflow:** PRDs scored by checks ([PM document optimizer](https://github.com/lifang-mban/pm-document-optimizer)); GitHub issues taken through implement → review → fix behind a score gate ([smallnest/autoresearch](https://github.com/smallnest/autoresearch)); browser strategies graduated into skills ([autobrowse](https://skills.sh/browserbase/skills/autobrowse)).
- **Mathematics:** conjecture generation ([Moonshine](https://arxiv.org/abs/2606.10806)); the Erdős–Straus conjecture ([autoresearch_StrausConjecture](https://github.com/mescgit/autoresearch_StrausConjecture)).

## 3. Nine ways the loop changes shape

The last column says what theuth's design 140 does today. "§18" means the item is already on 140's own "Not here" list.

| Variant | What changes | Examples | theuth today |
|---|---|---|---|
| Non-code artefact on an eval set | prompts, skills, agent configs and documents, scored per case | Datadog, Langfuse, [EvoSkill](https://github.com/sentient-agi/EvoSkill), autovoiceevals, [AutoPrompter](https://github.com/gauravvij/AutoPrompter) | works when the artefact is a file in a repo; per-case GEPA optimiser §18 |
| Metric lives in the world | live users, markets, robots, labs: slow, noisy, costly, cannot be reset | autoresearch-anything, [swarma](https://github.com/glitch-rabin/swarma), delu-agent, ENPIRE | not covered: the benchmark must be minutes long, offline and repeatable |
| Judged, no scalar | blind pairwise or tournament judging | [Autoreason](https://github.com/NousResearch/autoreason) (incumbent vs revision vs synthesis) | judge-only metrics warn and need a holdout; Elo tournaments §18 |
| Several objectives | keep a Pareto frontier, not one number plus guards | [GEPAResearch](https://github.com/CyrusNuevoDia/gepa-research), [Clio](https://github.com/zhongdaweiai/clio) | guards only; Pareto keeps §18 |
| Search, not a ratchet | tree search over worktrees, evolutionary ensembles, quality-diversity archives | [evo](https://github.com/evo-hq/evo), [EvE](https://github.com/scaling-group/eve), [Kernaut](https://github.com/richardcsuwandi/kernaut), [ShinkaEvolve](https://github.com/SakanaAI/ShinkaEvolve), [OpenEvolve](https://github.com/algorithmicsuperintelligence/openevolve) | ratchet + waves; archives §18 (the ledger already keeps parent, patch hash, category) |
| Many machines, many people | claim queue, shared ledger, verification before merge | [autoresearch-at-home](https://github.com/mutable-state-inc/autoresearch-at-home), [polyresearch](https://github.com/superagent-ai/polyresearch), [Agora](https://arxiv.org/abs/2609.18094) (13 workers, ~12 days, no planner), 35 agents / 333 experiments in one night ([minibase](https://www.minibase.md/blog/seti-at-home-for-ai-research-agenthub-autoresearch/)) | one engine per study; sandbox and mesh slots §18 |
| Ideas from the literature | idea pools seeded from papers | Paper Lantern, [autoresearch-speedrun](https://github.com/PashaDolgirev/autoresearch-speedrun) (paired-seed funnel), [lawforge](https://github.com/PAMF2/lawforge) (arXiv scan + bandit, Lean 4) | in-study web and literature search §18 |
| The loop improves the loop | an outer loop rewrites the inner loop's search, or the agent harness | [Bilevel Autoresearch](https://arxiv.org/abs/2603.23420) (5×: −0.045 vs −0.009 val_bpb), [Self-Harness](https://arxiv.org/abs/2606.09498), [Mixture of Self-Improving Branches](https://arxiv.org/abs/2609.37834), MiniMax M2.7 | not planned; dreaming's replay gate is the nearest relative |
| Open-ended science | idea → experiments → paper, or conjectures; no fixed metric | [AutoResearchClaw](https://github.com/aiming-lab/AutoResearchClaw), [Sibyl](https://github.com/Sibyl-Research-Team/AutoResearch-SibylSystem), Moonshine | a different feature |

## 4. What goes wrong, and the common fix

- **Gaming and overfit are still the main failure.** A [write-up of Karpathy's original run](https://www.birjob.com/blog/karpathy-autoresearch-2026) separates Goodhart overfit (Liquid) from outright metric gaming (a Gomoku alpha-beta hack). Trading backtests are the most exposed: a doubled return in 20 minutes needs a holdout period before anyone trusts it.
- **Every serious variant adds a second, independent signal:**
  - a holdout (mmm-research reports holdout error);
  - paired seeds ([autoresearch-speedrun](https://github.com/PashaDolgirev/autoresearch-speedrun));
  - adversarial validation (Clio);
  - a judge the agent cannot touch ([NanoTLM](https://github.com/Luisgarcav/nanotlm));
  - verified results before merge (polyresearch);
  - evidence gates and leave-one-out validation ([Vesuvius AutoResearch](https://github.com/mojomast/vesuvius-autoresearch)).

## 5. What this suggests for theuth (candidates, not decisions)

1. **Artefact studies on eval sets.** This is the biggest use outside ML training, and the closest to what 140 already stores (`CASE` lines). A GEPA-style per-case optimiser is the cheapest step. theuth's own prompts and skills are an obvious first target, which is the Self-Harness idea applied at home.
2. **Swarm slots on the mesh and sandbox VMs.** polyresearch and Agora show the coordination shape: a claim queue, an append-only shared ledger, verification before merge. theuth already has the mesh and the sandboxes.
3. **Pareto keeps and judge tournaments,** for objectives that have no single number worth trusting.
4. **Metrics that live in the world** look like a separate feature: a routine that runs exposure-limited experiments and reads results days later, not an extension of the minutes-long study loop.