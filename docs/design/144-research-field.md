# Survey C — what makes an AI notebook feel like a notebook (field, 2026-10-08)

Scope: NotebookLM (renamed Gemini Notebook on 2026-07-16), lfnovo/open-notebook, comparable open-source projects, and practitioner opinion. Every claim carries the URL it rests on; a `## Sources` list closes the file. Method notes: `search_web` was degraded (Brave rate-limited, Google 403, Qwant captcha), so most evidence comes from direct reads of Google's own blog/help/Workspace-updates pages, the GitHub REST/search API for open-notebook and comparable repos, and the HN Algolia API. Reddit threads were blocked (HTTP 403) and are cited only through secondary summaries, marked as such.

## 1. NotebookLM's feature set as of 2026-10

**Identity and timeline.** Google renamed NotebookLM to **Gemini Notebook** on 2026-07-16; it "remains a standalone product focused on being your premier research tool" and the old links redirect ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/07/notebooklm-now-gemini-notebook.html); [Keyword](https://blog.google/innovation-and-ai/products/gemini-notebook/notebooklm-gemini-notebook/)). An independent release tracker (updated 2026-08-25) lists 17 official 2026 changes by month ([notebooklm-guide tracker](https://notebooklm-guide.com/notebooklm-updates/)); the dates below are taken from Google's own posts where they could be read, and from that tracker otherwise.

**Dated timeline (2024-09 → 2026-09), newest last.**

| Date | What shipped | Source |
|---|---|---|
| 2024-09 | Audio Overviews (two-host podcast) | [link](https://simonwillison.net/2024/Sep/29/notebooklm-audio-overview/) |
| 2025-04-02 | Discover sources + I'm Feeling Curious | [link](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-discover-sources/) |
| 2025-05-19 | Official iOS/Android app | [link](https://blog.google/innovation-and-ai/products/notebooklm-app/) |
| 2025-06-03 | Public notebooks via link (viewers ask, cannot edit sources) | [link](https://blog.google/technology/google-labs/notebooklm-public-notebooks/) |
| 2025-07-29 | Video Overviews; Studio holds multiple outputs per type; audio + mind map side by side | [link](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-video-overviews-studio-upgrades/) |
| 2025-09 | Flashcards, Quizzes, redesigned Reports (Workspace/Education) | [link](https://workspaceupdates.googleblog.com/2025/09/flashcards-quizzes-reports-notebook-lm-google-education.html) |
| 2025-11-06 | Flashcards/quizzes in the app; temporary source select/unselect; 4x context, 6x memory | [link](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-app-quizzes-flashcards/) |
| 2025-11-13 | Deep Research in the source panel; Sheets, Drive URLs, images, .docx | [link](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-deep-research-file-types/) |
| 2026-01 | Notebooks as grounded sources inside the Gemini app (Workspace) | [link](https://notebooklm-guide.com/notebooklm-updates/) |
| 2026-02 | AI Expanded Access raises usage for eligible Workspace users | [link](https://notebooklm-guide.com/notebooklm-updates/) |
| 2026-03-04 | Cinematic Video Overviews (AI Ultra, 18+) | [link](https://blog.google/innovation-and-ai/products/notebooklm/generate-your-own-cinematic-video-overviews-in-notebooklm/) |
| 2026-03-20 | Slide revisions, 10 infographic styles, saved flashcard progress, EPUB, PPTX export, saved private chat history | [link](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html) |
| 2026-04 | Notebooks inside the Gemini app; higher education limits; Classroom class notebooks | [link](https://notebooklm-guide.com/notebooklm-updates/) |
| 2026-05-26 | Automatic Drive source syncing; Schoology course sources; Ask NotebookLM step in Workspace Studio | [link](https://workspaceupdates.googleblog.com/2026/05/keep-your-sources-up-to-date-with-automatic-Drive-syncing-in-NotebookLM.html) |
| 2026-06-08 | Gemini 3.5 + per-notebook cloud computer (code execution, 100+ skills); agentic source discovery; reports/charts/spreadsheets/slides | [link](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/) |
| 2026-06-25 | Study Notebooks in Gemini sync with NotebookLM | [link](https://blog.google/innovation-and-ai/products/gemini-app/gemini-study-notebooks/) |
| 2026-07-16 | Renamed Gemini Notebook; notebook.google.com | [link](https://workspaceupdates.googleblog.com/2026/07/notebooklm-now-gemini-notebook.html) |
| 2026-08-07 | Workspace Studio step auto-adds sources | [link](https://notebooklm-guide.com/notebooklm-updates/) |
| 2026-08-17 | Copy an entire notebook (no chats/user notes carried) | [link](https://workspaceupdates.googleblog.com/2026/08/make-copy-of-notebook-in-gemini-notebook.html) |
| 2026-08-19 | Notebooks in Search AI Mode, synced | [link](https://blog.google/products-and-platforms/products/search/back-to-school-study-tools/) |
| 2026-08-27 | Expert Intelligence: Play Books ebooks as sources | [link](https://blog.google/innovation-and-ai/products/gemini-notebook/expert-intelligence-leading-sources/) |
| 2026-08-28 | Compute-based usage limits, 5-hour refresh, deferred heavy outputs (from 09-02) | [link](https://blog.google/innovation-and-ai/products/gemini-notebook/new-flexible-usage-limits/) |
| 2026-09-15 | New study tools | [link](https://blog.google/innovation-and-ai/products/gemini-notebook/new-study-tools-september-2026/) |

**Sources — types and limits.**
- Supported types: pasted text, Google Docs, Slides (≤ 100 slides), Sheets (≤ 100k tokens), images (avif/bmp/gif/heic/jpg/png/tif/webp…), Word (.docx), .txt, .md, PDF, CSV, audio (MP3/WAV…), YouTube and web URLs (text content only; footnotes/comments of Google files are not imported) ([Gemini Notebook Help: add or discover sources](https://support.google.com/gemininotebook/answer/16215270?hl=en&co=GENIE.Platform%3DDesktop)).
- Each source ≤ 500,000 words or 200 MB; free accounts get 50 sources per notebook ([Help](https://support.google.com/gemininotebook/answer/16215270?hl=en&co=GENIE.Platform%3DDesktop)). A third-party table checked 2026-10-05 reports per-plan caps of 50 / 100 / 300 / 500 / 600 sources and 100–500 notebooks (Standard → Ultra) citing Google's upgrade table ([Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)) — treat the plan numbers as secondary.
- 2025-11-13: Deep Research inside the source panel ("acts like a dedicated researcher, creating reports and recommending sources"), plus Sheets, Drive URLs, images, Drive PDFs and .docx ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-deep-research-file-types/)). 2026-03-20: EPUB upload ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html)). Chat context can be narrowed by temporarily selecting/unselecting sources ([Keyword, app update](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-app-quizzes-flashcards/)).
- 2026-05-26: Google Drive sources auto-sync when the original changes; revoked permissions and deletions are reflected in the notebook ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/05/keep-your-sources-up-to-date-with-automatic-Drive-syncing-in-NotebookLM.html)). Inactive (inaccessible) sources still count toward limits but are not referenced in chat or Studio ([Help](https://support.google.com/gemininotebook/answer/16215270?hl=en&co=GENIE.Platform%3DDesktop)).
- 2026-08-07: a Workspace Studio step adds text, Drive links, YouTube and web URLs to a notebook automatically ([tracker](https://notebooklm-guide.com/notebooklm-updates/)).

**Discover.** 2025-04-02: "Discover sources" — describe a topic, NotebookLM finds and summarizes web sources, one click adds them; an "I'm Feeling Curious" button explores random topics ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-discover-sources/)). 2026-06-08: "agentic source discovery" — start with a loose idea and the system finds and organizes web sources, with a reported 78.2 % win rate on web research/source discovery vs the prior system ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/)).

**Notes.** The product's own design story describes the three-column shape — Inputs (source list / open source) → Chat with citations → Outputs (notes list / open note), with four layouts: Standard, Reading + Chat, Chat + Writing, Reading + Writing ([Jason Spielman, Designing NotebookLM](https://jasonspielman.com/notebooklm)). Deleted notes cannot be recovered ([Help FAQ](https://support.google.com/gemininotebook/answer/16269187?hl=en)). Copying a notebook (2026-08-17) transfers eligible sources and Studio artifacts but *not* chat history or user-written notes, and the copy does not stay in sync ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/08/make-copy-of-notebook-in-gemini-notebook.html)).

**Chat with citations.** Answers carry inline numbered citations that resolve to a passage in the source (see §2). 2025-11 mobile chat: "50% better quality, a 4x larger context window and 6x longer conversation memory" ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-app-quizzes-flashcards/)). 2026-03-20: conversation history is now saved and private to each user — a session can be closed and resumed; in shared notebooks your chat is visible only to you ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html)). 2026-06-08: chat runs on Gemini 3.5 with a secure cloud computer that writes and runs code, "more than 100 curated software skills", and a reported 65 %+ average win rate across five evaluation dimensions ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/)).

**Audio Overview / Video Overview.** Audio Overviews (two-host podcast) launched 2024-09 and drove the first HN wave (907 points) ([Simon Willison](https://simonwillison.net/2024/Sep/29/notebooklm-audio-overview/); [HN Algolia](https://hn.algolia.com/api/v1/search?query=NotebookLM&tags=story)). Video Overviews — "narrated slides" — and a redesigned Studio that holds multiple outputs of the same type arrived 2025-07-29 ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-video-overviews-studio-upgrades/)). Cinematic Video Overviews (Gemini 3 + Nano Banana Pro + Veo 3, "Gemini acts as a creative director") launched 2026-03-04 for AI Ultra 18+ ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/generate-your-own-cinematic-video-overviews-in-notebooklm/)).

**Mind Maps, Reports, Flashcards, Quizzes, infographics, slides.** Flashcards, Quizzes and redesigned Reports rolled out to Workspace/Education in 2025-09 ([Workspace Updates](https://workspaceupdates.googleblog.com/2025/09/flashcards-quizzes-reports-notebook-lm-google-education.html)) and to the mobile app 2025-11-06 with topic/difficulty/count controls ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-app-quizzes-flashcards/)). 2026-03-20: flashcard/quiz progress saved across sessions, "Got it / Missed it", shuffle, rerun missed; slide revisions with per-slide feedback; ten infographic styles; PPTX export ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html)). 2026-06-08: custom reports, charts, spreadsheets and slide decks from sources ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/)). Mind maps are listed by Google as a Studio output you can explore while audio plays ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-video-overviews-studio-upgrades/)); a per-plan "mind-map" quota exists ([Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)).

**Sharing / publishing.** 2025-06-03: public notebooks via "Anyone with a link"; viewers cannot edit sources but can ask questions and open generated content; since 2025-08-01 public sharing is personal-accounts only, Workspace/Education share within the domain ([Keyword](https://blog.google/technology/google-labs/notebooklm-public-notebooks/)). 2026-08-17: copy an entire notebook when the owner allows ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/08/make-copy-of-notebook-in-gemini-notebook.html)). 2026-08-27: "Expert Intelligence" lets you add purchased Play Books ebooks as sources ([Keyword](https://blog.google/innovation-and-ai/products/gemini-notebook/expert-intelligence-leading-sources/)).

**Mobile.** Official iOS/Android app 2025-05-19, the "most frequent request": Audio Overviews on the go, questions in the moment, share-sheet into a notebook ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm-app/)); the help centre still flags mobile feature gaps ([Help FAQ](https://support.google.com/gemininotebook/answer/16269187?hl=en)).

**Ecosystem spread (2026).** Notebooks inside the Gemini app (2026-04), Study Notebooks in Gemini syncing with NotebookLM (2026-06-25, [Keyword](https://blog.google/innovation-and-ai/products/gemini-app/gemini-study-notebooks/)), notebooks in Search AI Mode (2026-08-19, [Keyword](https://blog.google/products-and-platforms/products/search/back-to-school-study-tools/)), Classroom and Schoology course sources ([tracker](https://notebooklm-guide.com/notebooklm-updates/)). Usage limits moved from daily counts to compute-based limits refreshing every five hours, with deferred generation of heavy outputs (2026-08-28, rolling out 2026-09-02) ([Keyword](https://blog.google/innovation-and-ai/products/gemini-notebook/new-flexible-usage-limits/); [Help](https://support.google.com/gemininotebook/answer/17670842)). New study tools announced 2026-09-15 ([Keyword](https://blog.google/innovation-and-ai/products/gemini-notebook/new-study-tools-september-2026/)).

**API / enterprise.** Consumer NotebookLM has no public API. Gemini Notebook Enterprise (Google Cloud) exposes a REST `v1alpha` Discovery Engine API: `notebooks.create`, get/list notebooks, add sources, with CMEK ([Google Cloud docs](https://docs.cloud.google.com/gemini/enterprise/notebooklm-enterprise/docs/api-notebooks)). The June 2026 agentic upgrade launched for AI Ultra and specific Workspace business accounts first, Pro web in July ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/); [tracker](https://notebooklm-guide.com/notebooklm-updates/)).

## 2. How NotebookLM's citations work from the user's view

**Mechanics.** Each chat answer carries inline numbered chips; clicking one opens the source in the left/reading panel and highlights the cited passage — "Every AI response includes inline citations you can click to jump to the exact passage in the original document" ([aitooldiscovery Reddit digest, secondary](https://www.aitooldiscovery.com/guides/notebooklm-reddit)). Google's own FAQ confirms the passage-level model by stating its exception: "If your source content is too short, Gemini Notebook references the entire document without a cited individual text from your source" ([Help FAQ](https://support.google.com/gemininotebook/answer/16269187?hl=en)). The designer describes the "Reading + Chat" layout as "optimized for referencing sources and generating responses with citations" — i.e. the open source and the chat sit side by side so a chip can light a passage ([Spielman](https://jasonspielman.com/notebooklm)).

**Grounding is the pitch.** The product only uses the sources you add; a Reddit summary of the subreddit's view: "It does not pretend to know things outside your sources. Every answer comes with a citation you can click" ([secondary digest](https://www.aitooldiscovery.com/guides/notebooklm-reddit)). Google's 2026 posts keep calling outputs "source-grounded" ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/)).

**What people praise.**
- Confidence and team use: "the citations that NotebookLM provide increase the confidence in answers. This allows to use NotebookLM for team-level knowledge bases" ([HN comment, 2025-09](https://news.ycombinator.com/item?id=45316814)).
- Consulting workflows: loading dozens of SOWs/transcripts and asking questions "like I would if I were starting from scratch with a customer… It answered the questions with citations" ([HN comment, 2025-01](https://news.ycombinator.com/item?id=42597498)); "inline foot notes with citations to your documentation" across 20–25 documents is the thing people point to when they say it is more than podcasts ([HN comment, 2025-02](https://news.ycombinator.com/item?id=42898856)).
- Trust posture: "Don't trust any facts from an LLM and use your own trusted source of information and RAG where it will give you citations… I use NotebookLM for most of my real world work these days with my project documentation" ([HN comment, 2025-01](https://news.ycombinator.com/item?id=42674249)).

**What people dislike or work around.**
- Citations do not survive copy-out. A Chrome extension exists solely to "map citation numbers to source filenames" and "copy chat text with preserved citations" as `[1]`, `[2]` plus a source legend ([nicremo/notebookLM-citation](https://github.com/nicremo/notebookLM-citation)); another HN-posted tool converts "inline numbered references" into academic citations and bibliographies for Word/PDF export ([HN comment, 2026-05](https://news.ycombinator.com/item?id=48073546)). A 2024 subreddit thread is literally titled "Citations in saved notes" (could not be read: HTTP 403) ([Reddit](https://www.reddit.com/r/notebooklm/comments/1ge2v7z/citations_in_saved_notes/)).
- Short sources cite the whole document, not a passage ([Help FAQ](https://support.google.com/gemininotebook/answer/16269187?hl=en)); extraction failures silently shrink what can be cited ("protected or poorly extracted files can still import incompletely") ([Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)).
- No cross-notebook citations: "separate notebooks still cannot search each other" and upgrading "does not turn separate notebooks into one retrieval context" ([Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)); Reddit digest cons list "No cross-notebook querying: each notebook is isolated" and "Response quality drops for very long or complex source documents" ([secondary digest](https://www.aitooldiscovery.com/guides/notebooklm-reddit)).
- Even when grounded the answer can still be wrong about the text: a reader testing an obscure French novel found it "passable at summarizing chapters" but checked the symbolism answers against the book ([HN comment, 2026-01](https://news.ycombinator.com/item?id=46611126)).

## 3. open-notebook (lfnovo/open-notebook)

**Project state.** MIT, created 2024-10-21, 39,941 stars, 4,622 forks, 151 open issues, last push 2026-10-05; described as "An Open Source implementation of Notebook LM with more flexibility and features" ([GitHub API](https://api.github.com/repos/lfnovo/open-notebook); [repo](https://github.com/lfnovo/open-notebook)). Latest release 1.15.0 on 2026-10-04 ([CHANGELOG](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)). HN traction is nil: the Algolia API returns four stories for the project (2025-10-08, 2025-12-07, 2026-01-24 plus the website) with no recorded points or comments ([HN Algolia](https://hn.algolia.com/api/v1/search?query=open-notebook&tags=story)).

**Most-upvoted open issues** (GitHub search API, sort=reactions, 2026-10-08) ([API query](https://api.github.com/repos/lfnovo/open-notebook/issues?state=open&sort=reactions&per_page=30)):

| # | 👍 | comments | opened | title |
|---|---|---|---|---|
| [#712](https://github.com/lfnovo/open-notebook/issues/712) | 10 | 11 | 2026-04 | [Umbrella] Multi-user System Support |
| [#203](https://github.com/lfnovo/open-notebook/issues/203) | 11 | 10 | 2025-10 | [Vision] Content Modes & Artifacts (quiz/flashcards/mind maps/infographics/slides) |
| [#366](https://github.com/lfnovo/open-notebook/issues/366) | 5 | 4 | 2025-12 | Kubernetes deployment support |
| [#342](https://github.com/lfnovo/open-notebook/issues/342) | 4 | 2 | 2025-12 | Search the web for new sources using local SearXNG |
| [#855](https://github.com/lfnovo/open-notebook/issues/855) | 4 | 2 | 2026-06 | Fetching sources from Zotero |
| [#324](https://github.com/lfnovo/open-notebook/issues/324) | 3 | 1 | 2025-12 | Plugin architecture |
| [#331](https://github.com/lfnovo/open-notebook/issues/331) | 2 | 2 | 2025-12 | Multimodal / vision strategy (image OCR + VLM) |
| [#559](https://github.com/lfnovo/open-notebook/issues/559) | 2 | 5 | 2026-02 | Google OAuth provider connection |
| [#257](https://github.com/lfnovo/open-notebook/issues/257) | 2 | 14 | 2025-11 | Duplicate source detection and warning |
| [#294](https://github.com/lfnovo/open-notebook/issues/294) | 0 | 5 | 2025-12 | Provide better links to document references (cite paragraphs, not whole documents) |
| [#878](https://github.com/lfnovo/open-notebook/issues/878) | 1 | 2 | 2026-06 | MCP server for Open Notebook ("the memory loop") |
| [#1087](https://github.com/lfnovo/open-notebook/issues/1087) | 1 | 2 | 2026-07 | Support rerankers |
| [#1162](https://github.com/lfnovo/open-notebook/issues/1162) | 1 | 2 | 2026-07 | Mobile app |
| [#936](https://github.com/lfnovo/open-notebook/issues/936) | 1 | 2 | 2026-06 | Export notebook / sources to PDF |
| [#547](https://github.com/lfnovo/open-notebook/issues/547) | 0 | 2 | 2026-02 | Interactive podcast mode — ask questions during playback |
| [#662](https://github.com/lfnovo/open-notebook/issues/662) | 0 | 2 | 2026-03 | Sources and notebooks collections and tags |

**Most-upvoted closed issues** (same API, state=closed): SSO/OIDC multi-user ([#277](https://github.com/lfnovo/open-notebook/issues/277) 13 reactions, [#607](https://github.com/lfnovo/open-notebook/issues/607) 11), LaTeX ([#606](https://github.com/lfnovo/open-notebook/issues/606) 7, shipped 1.10.0), Chinese UI / i18n ([#349](https://github.com/lfnovo/open-notebook/issues/349), [#344](https://github.com/lfnovo/open-notebook/issues/344)), infographics/slides ([#347](https://github.com/lfnovo/open-notebook/issues/347), folded into #203), "select notebook in ask and search" ([#87](https://github.com/lfnovo/open-notebook/issues/87)), local TTS/STT ([#85](https://github.com/lfnovo/open-notebook/issues/85)), OpenAI-compatible embeddings ([#117](https://github.com/lfnovo/open-notebook/issues/117)), model list per provider ([#275](https://github.com/lfnovo/open-notebook/issues/275)), mind-map visualisation ([#706](https://github.com/lfnovo/open-notebook/issues/706), folded into #203), "Chat with Notebook defaults sources to Insights" ([#504](https://github.com/lfnovo/open-notebook/issues/504)), headless API ([#42](https://github.com/lfnovo/open-notebook/issues/42)), note-editor dialog not scrollable / save unreachable ([#511](https://github.com/lfnovo/open-notebook/issues/511)).

**Most-commented issues are installation and connectivity**, not features: Docker behind a reverse proxy ([#179](https://github.com/lfnovo/open-notebook/issues/179), 46 comments), Ollama on Ubuntu ([#64](https://github.com/lfnovo/open-notebook/issues/64), 30), "Unable to connect to the server" ([#159](https://github.com/lfnovo/open-notebook/issues/159), 27), "Failed to send messages" ([#345](https://github.com/lfnovo/open-notebook/issues/345), 19; [#358](https://github.com/lfnovo/open-notebook/issues/358), 16), "Unable to Connect to API Server" ([#316](https://github.com/lfnovo/open-notebook/issues/316), 17), Ollama behaving strangely ([#249](https://github.com/lfnovo/open-notebook/issues/249), 16), offline use / tiktoken ([#264](https://github.com/lfnovo/open-notebook/issues/264), 14), Nginx + Authelia ([#201](https://github.com/lfnovo/open-notebook/issues/201), 12), SurrealDB PermissionDenied ([#409](https://github.com/lfnovo/open-notebook/issues/409), 12), OpenAI-compatible `.env` config ([#90](https://github.com/lfnovo/open-notebook/issues/90), 11), LM Studio/Ollama over HTTPS ([#274](https://github.com/lfnovo/open-notebook/issues/274), 11), 10 MB source body limit ([#361](https://github.com/lfnovo/open-notebook/issues/361), 9), API timeouts ([#131](https://github.com/lfnovo/open-notebook/issues/131), 9), podcast "Invalid json output" ([#211](https://github.com/lfnovo/open-notebook/issues/211), 9).

**Recurring complaint clusters, with the maintainers' own words.**
- *Installation/config drift.* The unreleased changelog admits docs "described an older or imagined product" and were rewritten (≈ 6,200 → 1,450 lines), that `API_CLIENT_TIMEOUT` "was never read", that the compose file "doesn't load `.env` into the container", and that "Invented features are gone (tags, export/share, batch transformations, page-number citations and others)" ([CHANGELOG, Unreleased](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)).
- *SurrealDB.* v3 compatibility is an open tracked item ([#378](https://github.com/lfnovo/open-notebook/issues/378)); the VISION's "Platform v-next" cluster bundles the SurrealDB v3 migration, a frontend/backend image split and a possible Surreal Commands → Celery move "as one coordinated breaking change" ([VISION.md](https://github.com/lfnovo/open-notebook/blob/main/VISION.md)); namespace/database env mis-set makes job status read "unknown" ([#1466](https://github.com/lfnovo/open-notebook/issues/1466)).
- *Model/provider config.* 24 providers in the registry; "Auto-assign Defaults" picks outdated models ([#1473](https://github.com/lfnovo/open-notebook/issues/1473)); Azure credentials lack an API-version field ([#1470](https://github.com/lfnovo/open-notebook/issues/1470)); an umbrella for advanced params/presets/health exists ([#947](https://github.com/lfnovo/open-notebook/issues/947)); timeouts were misreported as connection failures until the unreleased fix ([CHANGELOG](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)).
- *Editor/notes.* Note-editor dialog could not scroll to Save ([#511](https://github.com/lfnovo/open-notebook/issues/511)); users ask for paragraph-level citations rather than whole-document references ([#294](https://github.com/lfnovo/open-notebook/issues/294)); VISION declares the product is **not** a document editor ("use Google Docs, Notion") ([VISION.md](https://github.com/lfnovo/open-notebook/blob/main/VISION.md)).
- *Performance/long jobs.* Frontend request timeout had to become configurable (default 10 min) for slow models ([CHANGELOG 1.10.0](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)); podcast progress reporting ([#694](https://github.com/lfnovo/open-notebook/issues/694)); embedding batch size and min-chunk env knobs for CPU-only setups ([CHANGELOG 1.9.0](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)).

**Roadmap / vision.** VISION.md (reviewed 2026-07) fixes identity — privacy-first, provider-agnostic, API-first, async-first, "Simplicity over features" — and the current posture: "get the basics working well for everyone before expanding" (sources, chat, search, notes, podcasts). Four horizon clusters: Platform v-next (#372/#378/#381), Multi-user (#712, "Approved — design phase (targeted post-v2)"), Content modes & artifacts (#203), and "Agents operating Open Notebook" via MCP — "the platform becomes the research memory of agents, not just a UI" (#878/#693/#973) ([VISION.md](https://github.com/lfnovo/open-notebook/blob/main/VISION.md); [#712](https://github.com/lfnovo/open-notebook/issues/712); [#878](https://github.com/lfnovo/open-notebook/issues/878)).

**What shipped recently.** 1.15.0 (2026-10-04): ODT/ODS/ODP sources, YouTube proxy/cookie options, SiliconFlow and Z.ai providers, MiniMax TTS, and a "Quiet Green" design foundation with "evidence/citation classes, context-state colors" co-designed in Discussion #1202 ([CHANGELOG](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)). 1.14.0 (2026-07-20): Docling formula/vision enrichment toggles, OpenRouter TTS/STT. 1.10.0 (2026-06-17): KaTeX math in chat, bulk context actions — "Include all (insights only)", "Include all (full content)", "Exclude all from context" for sources and notes — and a fix where podcast generation "ran against empty context" because `Notebook.get_context()` was missing ([CHANGELOG](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)). Its context model is explicit per item: each source is in context as *insights only*, *full content* or excluded; Ask is described as three stages ([CHANGELOG, Unreleased](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)).

## 4. Comparable open-source projects

One line each on container model · source ingestion · context control · citations, only from what the README states (GitHub, 2026-10-08).

- **SurfSense** (MODSetter) — "The air-gapped, open-source NotebookLM alternative"; pivoted to a desktop app (AppImage/deb) and is retiring its hosted web app with an export window closing 2026-10-18 ("documents, folders, titles and chat threads all come across"). Container: workspaces/folders of local documents; ingestion: PDFs, Office files, images parsed on-device into SQLite under `~/.surfsense`; context control: "Pick some documents and a format"; citations: "every answer cites the source it came from"; Studio outputs: summary, briefing, slide deck, report, study guide, podcast ([README](https://github.com/MODSetter/SurfSense)).
- **Khoj** (khoj-ai) — personal AI with "agents with custom knowledge, persona, chat model and tools"; ingestion: PDFs, Markdown, Notion, Word, org-mode, images; surfaces: browser, Obsidian, Emacs, desktop, phone, WhatsApp; semantic search; automations ("personal newsletters and smart notifications"). Container model is the agent's knowledge set rather than a notebook; citation behaviour not stated in the README ([README](https://github.com/khoj-ai/khoj)).
- **Reor** (reorproject) — local-first "AI personal knowledge management" desktop app with an Obsidian-like Markdown editor: "Every note you write is chunked and embedded into an internal vector database… Related notes are connected automatically via vector similarity… LLM-powered Q&A does RAG on your corpus of notes." Container: a notes vault; context: whole corpus; citations not stated ([README](https://github.com/reorproject/reor)).
- **Perplexica → Vane** (ItzCrazyKns, 37.1k stars, renamed) — an answer engine, not a notebook: SearXNG web search with "cited sources", Speed/Balanced/Quality modes, "Pick your sources" (web, discussions, academic), file uploads, domain-restricted search; no persistent source container ([README](https://github.com/ItzCrazyKns/Perplexica)).
- **AnythingLLM** (Mintplex-Labs) — "Chat with your docs" with **workspaces** as the container (documents can be shared across workspaces), drag-and-drop uploads "and source citations", multi-user with permissions (Docker), agents, Memories, scheduled tasks, open-source Android app ([README](https://github.com/Mintplex-Labs/anything-llm)).
- **Morphik Core** (morphik-org) — a multimodal retrieval engine for developers (ingest, search "deep and shallow", transform), built around the claim that "Traditional RAG approaches… fail spectacularly in production" on visually rich documents; an engine, not an end-user notebook ([README](https://github.com/morphik-org/morphik-core)).
- **RAGFlow** (infiniflow, 1.0.0-rc1 on 2026-09-29) — knowledge bases with "Visualization of text chunking to allow human intervention" and "traceable citations"; 2026-08-19 "Knowledge Compilation" generates Wikis, Graphs, Trees, PageIndex, Mind Maps, Timelines and Skills at document and dataset level; sitemap ingestion 2026-09-10 ([README](https://github.com/infiniflow/ragflow)).
- **Onyx** (onyx-dot-app) — "the knowledge/context layer for your team and AI agents": 50+ connectors with permissions, custom agents over "unique subsets of knowledge", web search, deep research, Slack/Discord bots and an MCP server so "Claude Code, Open Code, Codex, or any MCP client" get company context "with the same access controls" ([README](https://github.com/onyx-dot-app/onyx)).

Pattern: the projects that *feel* like notebooks (SurfSense, AnythingLLM, open-notebook) share three things — a named container that holds sources *and* chats, per-source inclusion control, and answers that cite back into those sources — and the three with the most momentum in 2026 (open-notebook, Onyx, RAGFlow) are all adding an MCP/agent surface so the notebook becomes memory for agents ([VISION.md](https://github.com/lfnovo/open-notebook/blob/main/VISION.md); [Onyx README](https://github.com/onyx-dot-app/onyx); [RAGFlow README](https://github.com/infiniflow/ragflow)).

## 5. Practitioner views: useful vs toy

- **Citations that resolve are the usefulness test.** Practitioners describing real work (consulting SOWs, project documentation, team knowledge bases) name the inline, clickable citation as the reason they trust the output enough to use it ([HN 2025-01](https://news.ycombinator.com/item?id=42597498); [HN 2025-09](https://news.ycombinator.com/item?id=45316814); [HN 2025-01](https://news.ycombinator.com/item?id=42674249)). The absence of a reliable citation path in copied text spawned third-party extensions ([nicremo](https://github.com/nicremo/notebookLM-citation); [HN 2026-05](https://news.ycombinator.com/item?id=48073546)).
- **Context control beats magic retrieval.** NotebookLM added temporary source (de)selection because "the response is only based on the sources you care about right now" ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-app-quizzes-flashcards/)); open-notebook's most concrete UX work in 2026 is per-item context state (insights-only / full / excluded) and bulk context actions ([CHANGELOG 1.10.0](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)), after a bug where generation silently ran on empty context ([CHANGELOG 1.10.0](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)). Users asked that chat default to Insights rather than full text ([#504](https://github.com/lfnovo/open-notebook/issues/504)).
- **Persistence of sources is the moat and the cage.** Living notebooks need auto-synced sources (Drive sync 2026-05, [Workspace Updates](https://workspaceupdates.googleblog.com/2026/05/keep-your-sources-up-to-date-with-automatic-Drive-syncing-in-NotebookLM.html)) and saved chat history (2026-03, [Workspace Updates](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html)); but a notebook that cannot be exported or duplicated with its notes and chats (copy loses both; deleted notes unrecoverable) is called a "one bounded source set" by a competitor's analysis ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/08/make-copy-of-notebook-in-gemini-notebook.html); [Help FAQ](https://support.google.com/gemininotebook/answer/16269187?hl=en); [Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)).
- **Cross-notebook ask is the most-cited structural gap** — "each notebook is isolated" ([Reddit digest](https://www.aitooldiscovery.com/guides/notebooklm-reddit)); "separate notebooks still cannot search each other" ([Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)); open-notebook users asked to "select notebook in ask and search" ([#87](https://github.com/lfnovo/open-notebook/issues/87)) and for collections/tags across notebooks ([#662](https://github.com/lfnovo/open-notebook/issues/662)). Google's answer so far is syncing the *same* notebook into Gemini, Search AI Mode and Workspace, not querying across notebooks ([tracker](https://notebooklm-guide.com/notebooklm-updates/); [Keyword](https://blog.google/products-and-platforms/products/search/back-to-school-study-tools/)).
- **Generated overviews are the hook, grounding is the retention.** Audio Overviews made the product famous (907-point HN thread; "surprisingly effective" — [Willison](https://simonwillison.net/2024/Sep/29/notebooklm-audio-overview/)) and a teacher used them with an 8-year-old ([HN](https://news.ycombinator.com/item?id=41688568)), but the HN comments that describe sustained work talk about citations across 20–25 documents, not podcasts ([HN](https://news.ycombinator.com/item?id=42898856); [HN](https://news.ycombinator.com/item?id=42597498)). Google's 2026 roadmap moved from more output formats (video, infographics, slides) toward agentic research with code execution and report/chart/spreadsheet outputs ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/)).
- **Study loop features need state.** Flashcards and quizzes only became habit-forming once progress saved across sessions with Got it / Missed it and rerun ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html)); Study Notebooks in Gemini add diagnostics and progress tracking over the same sources ([Keyword](https://blog.google/innovation-and-ai/products/gemini-app/gemini-study-notebooks/)).
- **Self-hosters' pain is operational, not conceptual.** In open-notebook, the 15 most-commented issues are Docker/proxy/connection/provider setup ([#179](https://github.com/lfnovo/open-notebook/issues/179), [#159](https://github.com/lfnovo/open-notebook/issues/159), [#316](https://github.com/lfnovo/open-notebook/issues/316)); the maintainers rewrote the docs "against the code" and deleted described-but-nonexistent features including "page-number citations" ([CHANGELOG](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)). A notebook that promises citations and cannot deliver them precisely is noticed.
- **Agents as notebook users.** open-notebook's horizon names "Agents operating Open Notebook… the research memory of agents" ([VISION.md](https://github.com/lfnovo/open-notebook/blob/main/VISION.md)); Onyx ships an MCP server for coding agents ([README](https://github.com/onyx-dot-app/onyx)); NotebookLM gave each notebook its own "secure cloud computer" and skills ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/)). Practitioners increasingly expect the notebook to be both a UI and a tool surface.

## 6. Takeaways for a product with an agent, block notes, collaborative editing, a knowledge base, meetings → notes, voice notes and web fetch

Ranked by how visibly users would notice, each with the evidence it rests on.

1. **Citations as first-class blocks that resolve to a highlighted passage — and survive copy, save and export.** The clickable chip → passage highlight is *the* NotebookLM behaviour people name when they explain why they trust it ([HN](https://news.ycombinator.com/item?id=45316814); [HN](https://news.ycombinator.com/item?id=42597498); [digest](https://www.aitooldiscovery.com/guides/notebooklm-reddit)); its single biggest workaround industry is preserving citations when text leaves the chat ([nicremo](https://github.com/nicremo/notebookLM-citation); [HN](https://news.ycombinator.com/item?id=48073546)); open-notebook's users ask for paragraph-level rather than document-level references ([#294](https://github.com/lfnovo/open-notebook/issues/294)) and its maintainers had to delete "page-number citations" from docs because they did not exist ([CHANGELOG](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)). For block notes this means: a citation is a block-level reference (source id + span) that stays attached when the block is moved, copied into another note or exported.
2. **A visible, per-source context switch on every ask (in / insights-only / out), with bulk actions.** NotebookLM shipped temporary select/unselect because answers should rest "only on the sources you care about right now" ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-app-quizzes-flashcards/)); open-notebook built three-state per-item context plus column-level bulk toggles and found a bug where generation ran on empty context ([CHANGELOG 1.10.0](https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md)); users want the cheaper insights mode as the default ([#504](https://github.com/lfnovo/open-notebook/issues/504)). Show the context state in the UI; never let an answer run on an empty or surprising context silently.
3. **A named container that holds sources, chats, notes and generated artifacts together — "Inputs → Chat → Outputs" in one view.** The product's designer frames the whole UX as that linear-but-flexible flow with sources and notes always reachable ([Spielman](https://jasonspielman.com/notebooklm)); every notebook-like OSS peer (SurfSense workspaces, AnythingLLM workspaces, open-notebook notebooks) keeps the same shape ([SurfSense](https://github.com/MODSetter/SurfSense); [AnythingLLM](https://github.com/Mintplex-Labs/anything-llm); [VISION.md](https://github.com/lfnovo/open-notebook/blob/main/VISION.md)). A note that is *about* a set of sources should show them.
4. **Cross-notebook / cross-container ask.** The most repeated structural complaint about NotebookLM ("each notebook is isolated"; "cannot search each other") ([digest](https://www.aitooldiscovery.com/guides/notebooklm-reddit); [Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)) and an early open-notebook request ("select notebook in ask and search", [#87](https://github.com/lfnovo/open-notebook/issues/87); collections/tags, [#662](https://github.com/lfnovo/open-notebook/issues/662)). A product with a knowledge base already has the substrate; exposing "ask across these notebooks" with citations into each is a differentiator Google has not shipped.
5. **Living sources: auto-refresh, provenance and a visible freshness state.** Drive auto-sync (2026-05) and reflected deletions/permission loss were a headline Workspace update ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/05/keep-your-sources-up-to-date-with-automatic-Drive-syncing-in-NotebookLM.html)); inactive sources are shown and excluded ([Help](https://support.google.com/gemininotebook/answer/16215270?hl=en&co=GENIE.Platform%3DDesktop)); open-notebook users want duplicate detection ([#257](https://github.com/lfnovo/open-notebook/issues/257)) and Zotero/SearXNG feeds ([#855](https://github.com/lfnovo/open-notebook/issues/855); [#342](https://github.com/lfnovo/open-notebook/issues/342)). With web fetch and meetings → notes already present, a source card that says *fetched 3 days ago · changed since · re-fetch* would be noticed.
6. **Discover / Deep Research into the container, not into the chat.** "Describe your topic… add with one click" (2025-04), Deep Research reports that land as sources (2025-11), agentic source discovery (2026-06) ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-discover-sources/); [Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-deep-research-file-types/); [Keyword](https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/)); open-notebook's fourth most-upvoted open issue asks for exactly this via local SearXNG ([#342](https://github.com/lfnovo/open-notebook/issues/342)). The agent already fetches the web; the notebook move is to let a research run *deposit* cited sources into the note's container.
7. **Persistent, private chat history attached to the notebook; shared notebook, per-person chat.** NotebookLM made saved history and "in shared notebooks, your chat is visible only to you" a 2026 feature ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html)); copying a notebook drops chats and user notes, which is the complaint ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/08/make-copy-of-notebook-in-gemini-notebook.html); [Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)). Collaborative block editing already exists — pair it with per-member private chat threads whose answers can be promoted into shared blocks (with citations, per item 1).
8. **Generated overviews as artifacts of the notebook (briefing, FAQ, timeline, mind map, audio), regenerable and multiple.** Studio holds several outputs of the same type and lets you tailor each (language, role, chapter) ([Keyword](https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-video-overviews-studio-upgrades/)); flashcards/quizzes only stuck once progress persisted ([Workspace Updates](https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html)); open-notebook's second most-upvoted issue is a coherent "content modes & artifacts" surface rather than scattered buttons ([#203](https://github.com/lfnovo/open-notebook/issues/203)). Voice notes and meeting notes are already inputs; the audio overview is the output users recognise ([Willison](https://simonwillison.net/2024/Sep/29/notebooklm-audio-overview/)).
9. **Export and duplication that keep notes, chats and citations.** No note recovery, no copy of notes/chat, no synchronized export are the listed NotebookLM pitfalls ([Help FAQ](https://support.google.com/gemininotebook/answer/16269187?hl=en); [Workspace Updates](https://workspaceupdates.googleblog.com/2026/08/make-copy-of-notebook-in-gemini-notebook.html); [Atlas](https://www.atlasworkspace.ai/blog/notebooklm-limitations)); open-notebook users ask for PDF export of notebook and sources ([#936](https://github.com/lfnovo/open-notebook/issues/936)) and SurfSense's shutdown migrates "documents, folders, titles and chat threads" as a bundle ([SurfSense](https://github.com/MODSetter/SurfSense)).
10. **The notebook as a tool surface for agents (MCP/API).** open-notebook's horizon ("research memory of agents", [VISION.md](https://github.com/lfnovo/open-notebook/blob/main/VISION.md); [#878](https://github.com/lfnovo/open-notebook/issues/878)), Onyx's MCP server ([README](https://github.com/onyx-dot-app/onyx)), Gemini Notebook Enterprise's REST API ([Cloud docs](https://docs.cloud.google.com/gemini/enterprise/notebooklm-enterprise/docs/api-notebooks)) and open-notebook's long-standing "headless via API" request ([#42](https://github.com/lfnovo/open-notebook/issues/42)) all point the same way. A product whose agent already has tools should let that agent read, cite and write into notebooks through the same block/citation model users see.

Not recommended from this evidence: video/cinematic overviews (Ultra-only, no user demand signal in the OSS trackers beyond #203's slides/infographics), and multi-user SaaS features beyond what collaborative editing already gives (open-notebook's #712 is a self-hoster request for auth, not a notebook-feel request) ([Keyword](https://blog.google/innovation-and-ai/products/notebooklm/generate-your-own-cinematic-video-overviews-in-notebooklm/); [#712](https://github.com/lfnovo/open-notebook/issues/712)).

## Sources

Every URL used above (103), grouped.

**Google (Keyword, Workspace Updates, Help, Cloud docs)**
- https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-app-quizzes-flashcards/
- https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-deep-research-file-types/
- https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-discover-sources/
- https://blog.google/innovation-and-ai/models-and-research/google-labs/notebooklm-video-overviews-studio-upgrades/
- https://blog.google/innovation-and-ai/products/gemini-app/gemini-study-notebooks/
- https://blog.google/innovation-and-ai/products/gemini-notebook/expert-intelligence-leading-sources/
- https://blog.google/innovation-and-ai/products/gemini-notebook/new-flexible-usage-limits/
- https://blog.google/innovation-and-ai/products/gemini-notebook/new-study-tools-september-2026/
- https://blog.google/innovation-and-ai/products/gemini-notebook/notebooklm-gemini-notebook/
- https://blog.google/innovation-and-ai/products/notebooklm-app/
- https://blog.google/innovation-and-ai/products/notebooklm/better-research-notebooklm/
- https://blog.google/innovation-and-ai/products/notebooklm/generate-your-own-cinematic-video-overviews-in-notebooklm/
- https://blog.google/products-and-platforms/products/search/back-to-school-study-tools/
- https://blog.google/technology/google-labs/notebooklm-public-notebooks/
- https://docs.cloud.google.com/gemini/enterprise/notebooklm-enterprise/docs/api-notebooks
- https://support.google.com/gemininotebook/answer/16215270?hl=en&co=GENIE.Platform%3DDesktop
- https://support.google.com/gemininotebook/answer/16269187?hl=en
- https://support.google.com/gemininotebook/answer/17670842
- https://workspaceupdates.googleblog.com/2025/09/flashcards-quizzes-reports-notebook-lm-google-education.html
- https://workspaceupdates.googleblog.com/2026/03/new-ways-to-customize-and-interact-with-your-content-in-NotebookLM.html
- https://workspaceupdates.googleblog.com/2026/05/keep-your-sources-up-to-date-with-automatic-Drive-syncing-in-NotebookLM.html
- https://workspaceupdates.googleblog.com/2026/07/notebooklm-now-gemini-notebook.html
- https://workspaceupdates.googleblog.com/2026/08/make-copy-of-notebook-in-gemini-notebook.html

**open-notebook (GitHub repo, API, issues)**
- https://api.github.com/repos/lfnovo/open-notebook
- https://api.github.com/repos/lfnovo/open-notebook/issues?state=open&sort=reactions&per_page=30
- https://github.com/lfnovo/open-notebook
- https://github.com/lfnovo/open-notebook/blob/main/CHANGELOG.md
- https://github.com/lfnovo/open-notebook/blob/main/VISION.md
- https://github.com/lfnovo/open-notebook/issues/1087
- https://github.com/lfnovo/open-notebook/issues/1162
- https://github.com/lfnovo/open-notebook/issues/117
- https://github.com/lfnovo/open-notebook/issues/131
- https://github.com/lfnovo/open-notebook/issues/1466
- https://github.com/lfnovo/open-notebook/issues/1470
- https://github.com/lfnovo/open-notebook/issues/1473
- https://github.com/lfnovo/open-notebook/issues/159
- https://github.com/lfnovo/open-notebook/issues/179
- https://github.com/lfnovo/open-notebook/issues/201
- https://github.com/lfnovo/open-notebook/issues/203
- https://github.com/lfnovo/open-notebook/issues/211
- https://github.com/lfnovo/open-notebook/issues/249
- https://github.com/lfnovo/open-notebook/issues/257
- https://github.com/lfnovo/open-notebook/issues/264
- https://github.com/lfnovo/open-notebook/issues/274
- https://github.com/lfnovo/open-notebook/issues/275
- https://github.com/lfnovo/open-notebook/issues/277
- https://github.com/lfnovo/open-notebook/issues/294
- https://github.com/lfnovo/open-notebook/issues/316
- https://github.com/lfnovo/open-notebook/issues/324
- https://github.com/lfnovo/open-notebook/issues/331
- https://github.com/lfnovo/open-notebook/issues/342
- https://github.com/lfnovo/open-notebook/issues/344
- https://github.com/lfnovo/open-notebook/issues/345
- https://github.com/lfnovo/open-notebook/issues/347
- https://github.com/lfnovo/open-notebook/issues/349
- https://github.com/lfnovo/open-notebook/issues/358
- https://github.com/lfnovo/open-notebook/issues/361
- https://github.com/lfnovo/open-notebook/issues/366
- https://github.com/lfnovo/open-notebook/issues/378
- https://github.com/lfnovo/open-notebook/issues/409
- https://github.com/lfnovo/open-notebook/issues/42
- https://github.com/lfnovo/open-notebook/issues/504
- https://github.com/lfnovo/open-notebook/issues/511
- https://github.com/lfnovo/open-notebook/issues/547
- https://github.com/lfnovo/open-notebook/issues/559
- https://github.com/lfnovo/open-notebook/issues/606
- https://github.com/lfnovo/open-notebook/issues/607
- https://github.com/lfnovo/open-notebook/issues/64
- https://github.com/lfnovo/open-notebook/issues/662
- https://github.com/lfnovo/open-notebook/issues/694
- https://github.com/lfnovo/open-notebook/issues/706
- https://github.com/lfnovo/open-notebook/issues/712
- https://github.com/lfnovo/open-notebook/issues/85
- https://github.com/lfnovo/open-notebook/issues/855
- https://github.com/lfnovo/open-notebook/issues/87
- https://github.com/lfnovo/open-notebook/issues/878
- https://github.com/lfnovo/open-notebook/issues/90
- https://github.com/lfnovo/open-notebook/issues/936
- https://github.com/lfnovo/open-notebook/issues/947

**Comparable projects (GitHub)**
- https://github.com/ItzCrazyKns/Perplexica
- https://github.com/MODSetter/SurfSense
- https://github.com/Mintplex-Labs/anything-llm
- https://github.com/infiniflow/ragflow
- https://github.com/khoj-ai/khoj
- https://github.com/morphik-org/morphik-core
- https://github.com/nicremo/notebookLM-citation
- https://github.com/onyx-dot-app/onyx
- https://github.com/reorproject/reor

**Hacker News (threads, comments, Algolia API)**
- https://hn.algolia.com/api/v1/search?query=NotebookLM&tags=story
- https://hn.algolia.com/api/v1/search?query=open-notebook&tags=story
- https://news.ycombinator.com/item?id=41688568
- https://news.ycombinator.com/item?id=42597498
- https://news.ycombinator.com/item?id=42674249
- https://news.ycombinator.com/item?id=42898856
- https://news.ycombinator.com/item?id=45316814
- https://news.ycombinator.com/item?id=46611126
- https://news.ycombinator.com/item?id=48073546

**Other**
- https://jasonspielman.com/notebooklm
- https://notebooklm-guide.com/notebooklm-updates/
- https://simonwillison.net/2024/Sep/29/notebooklm-audio-overview/
- https://www.aitooldiscovery.com/guides/notebooklm-reddit
- https://www.atlasworkspace.ai/blog/notebooklm-limitations
- https://www.reddit.com/r/notebooklm/comments/1ge2v7z/citations_in_saved_notes/

Not reachable during this survey: r/notebooklm threads (HTTP 403 network block), support.google.com/gemininotebook/answer/16206239 (404), blog.google mind-maps post (404), acutive.com (HTTP 402), surfsense.com (empty shell). The `search_web` providers Brave/Google/Qwant were rate-limited or blocked; DuckDuckGo and direct reads supplied the rest.