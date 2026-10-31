# Calm redesign: Phase 1 evidence

Written 2026-10-31 against 0.9.7, before any redesign code. There are no
analytics, so the usage figures come from simulated users walking the real app
(see the agent-*.md walkthroughs) plus reading the code.

1. [Usage audit](1-usage-audit.md): every screen ranked by estimated use.
2. [Page forensics](2-page-forensics.md): why people struggle, as fix lists per screen.
3. [Numbers audit](3-numbers-audit.md): every figure on screen checked against its source.
4. [Route map](4-route-map.md): every screen and action that exists today. Nothing may disappear.
5. [Information architecture](5-information-architecture.md): Phase 3 plan. Areas, tabs, More, a home for every route, the orphan test, ⌘K search and the build order.

Screens referred to by name (main.png, compact.png...) were taken under Xvfb with
`--ui-test`; they're in the project's shared files, not the repo.
