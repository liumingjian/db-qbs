# db-qbs Design System · A Copy of the x2doris Admin Shell

> **Token file**: [`tokens.css`](tokens.css) — the single machine-readable source; every token
> records the measured x2doris value it came from.
> **Full-flow reference implementation**:
> [`../prototypes/0058-m2-full-flow-prototype.html`](../prototypes/0058-m2-full-flow-prototype.html)
> ([#58](https://github.com/liumingjian/db-qbs/issues/58)) — every component in this file used across
> the real six-stage flow, 4 screens and 18 states. It **inlines `tokens.css` verbatim** (CSP forbids
> `@import`): that is a copy, not a second set of tokens. **Change `tokens.css` first, then sync the
> prototype** — never the other way round.
>
> **This is a decision.** Every UI artifact from here on (prototypes and the production front end)
> reuses this file and **must not start a second system**. To change something, change it here rather
> than inventing it inside a single ticket.

## 0. In one sentence

**Copy the x2doris 1.2.0 job center** (the StreamPark lineage: vue-vben-admin + Ant Design 4):
**dark left nav + top-bar breadcrumb + grey content area + white cards wrapping tables**. Layout,
typography, UX, column contents, and inline icon actions all defer to it; token values come from
**measured computed styles**, not from numbers we made up.
**The one thing not copied is the font stack** — it is an English interface with no CJK fonts in it.
What is copied is the design, not the bugs: **you may skip a detail only if you can say exactly how
it is broken and your fix introduces no new shape.**

Three trade-offs that do not move:

1. **Blue means "clickable" only** (primary buttons, the selected nav item, action links) and carries
   no state semantics; state colour appears only on dots, tags, and alert boxes.
2. **A white card on a grey ground is the only layering device** — no stacked shadows, no gradients,
   no pill radii.
3. **Tables and numbers are the subject.** This is a tool for moving data, not a marketing page.

## 1. Tokens

[`tokens.css`](tokens.css) holds the values; this table only explains what each is for.

| Group | Tokens | Purpose |
|---|---|---|
| Layers | `--bg` `--panel` `--line` `--line-strong` | Grey ground / white card / divider / control border |
| Text | `--text` `--dim` `--mute` | Body and data / secondary notes / labels and units |
| Brand | `--brand` `--brand-dim` | **Clickable only**; `--brand-dim` is the selected-item background |
| Semantic | `--ok` `--crit` `--warn` `--info` (each with `-bg` / `-bd`) | Success / 4xx rejection / 5xx internal error / in progress |
| Shape | `--radius` 4px, `--badge-radius` 3px | Cards and controls / tags |
| Density | `--row-h` 44px, `--ctl-h` 30px, `--pad` 20px, `--gap` 16px | Moderately loose; a dozen-odd rows fit on one screen |
| Typography | `--font-cn` `--font-num` `--size-*` `--lh-*` `--weight-em` | See §2 |

## 2. CJK-first typography

**Under offline and CSP constraints a CJK webfont cannot be inlined, so a system font stack is the
only honest answer** — we do not pretend to ship a font.

- **Body stack**: `PingFang SC → HarmonyOS Sans SC → Source Han Sans SC → Noto Sans CJK SC →
  Microsoft YaHei → Hiragino Sans GB → sans-serif`. That covers macOS, HarmonyOS, Linux with Source
  Han installed, and Windows, falling back to the system sans-serif.
- **Numbers and identifiers use a separate monospace stack** with `font-variant-numeric: tabular-nums`:
  `run_id`, row counts, byte counts, durations, error codes, column names, SQL — all fixed-width,
  because **column alignment depends on it**. When CJK and digits are mixed, do not let the digits
  inherit the proportional figures of the CJK stack.
- **Line height**: `1.55` for interface body text; `1.7` for CJK paragraphs (error explanations,
  prose sections). CJK needs more leading than Latin.
- **Weight**: emphasis is always **600, never 700**. At 13–14px, CJK at 700 turns into mush.
- **No italics**: CJK has no true italic, and the browser's synthesised oblique is a broken glyph.
  Emphasise with weight or colour instead.
- **Size scale**: 14 body / 13.5 table / 13 numeric / 12 secondary / 11 field name. No finer.

## 3. The visual language of state and error

**Three shape axes that never borrow from one another.** Red/amber/green is not enough — when they
sit side by side on one screen you must be able to tell "how far did it get", "did the target table
move", and "why did it fail" apart at a glance.

| Axis | Semantic source | Shape | Notes |
|---|---|---|---|
| **Axis 1 · process phase** | The in-progress part of the run's five states | **Dot** + a short CJK phrase, strung as `PREPARING ─▶ STREAMING ─▶ COMMITTING` | Appears only while in progress; the current dot uses `--info` with an `--info-bg` halo, past phases use `--ok`, future ones `--mute` |
| **Axis 2 · resource final state** | The tombstone's two values | **Block**: `SWAPPED` solid (`--ok-fill` ground + `--ok-ink` text + `--ok-bd` border); `DISCARDED` neutral outline (**no fill** + 1px `--line-strong` border) | It answers **whether the target table moved**, and says nothing about why. A final-state block always carries a CJK gloss (`DISCARDED　目标表未被触碰`). Leaving `DISCARDED` unfilled is a hard requirement: give both a fill and, in greyscale, both become "a filled block with a border", and the "solid vs outline" promise of §8 collapses. **`SWAPPED` uses `--ok-fill`, not `--ok-bg`**: the latter is only 3.1% darker than white in greyscale, so the "solid" half would effectively be unpainted, and the promise needs both halves. `--ok-bd` sits at almost the same luminance as `--ok-fill`, making the border invisible — **that is correct**; a solid block should not need a border to stand up, so do not report it as a defect |
| **Axis 3 · error code** | The closed set of **12 codes** | **Tag**, monospace code name + small HTTP status, **dashed border for 4xx, solid for 5xx** | 4xx means "**you can fix this**" (the `--crit` family); 5xx means "**get help**" (the `--warn` family). The border shapes differ so they stay distinguishable under colour blindness and black-and-white printing |

**Hard rules**:

- The three shapes (dot / block / tag) **must never be swapped or reused**. A new state starts with
  "which axis is it on?" — if it is on none, settle its semantics in an ADR before drawing it.
- **Error codes are a closed set.** The UI must not invent codes, and must not show the HTTP status
  while swallowing the code name. Unknown codes render in the `INTERNAL_*` band with the string
  echoed verbatim.
- A **plain-language CJK conclusion** always follows the code name (the two-way split being: the
  verification numbers disagreed / verification never ran). Prose first, code second; the code is an
  anchor for someone who can look up the docs, not an explanation.
- **Axis 2 never appears in progress, axis 1 never appears at a final state.** Two axes on one row is
  allowed (final-state block + error tag); all three at once is not.
- **Axis 2 appears only when a tombstone genuinely exists.** Axis 2 draws the tombstone, and the
  tombstone is held by `sink`. In three situations `sink` has no such run at all and therefore no
  tombstone: **the SQL shape precheck failed** (no request was ever sent), **the mapping precheck
  failed** (`POST /runs` was rejected with 422, so no run resource exists), and the three
  **"outcome unknown"** cases (the parent process never saw an outcome and cannot know whether a
  tombstone exists). Those screens **never show a final-state block** — drawing a `DISCARDED` there
  would be **fabricating a record that does not exist**. The sentence "the target table was not
  touched" is demoted into the **plain-language conclusion bar**; the list's "outcome" column uses
  **neutral grey text** (`未发起` / `未建暂存表` / `结局不明`) rather than a block.
  The cost is accepted: one column then mixes blocks and grey text and looks uneven — **but that is
  the truth, and making it even would make it a lie.** Axis 1 works the same way (see the next rule):
  if the shape is wrong, do not draw it; never fill in a shape for the sake of a tidy layout.
- **The phase string draws three dots, not five.** The run's five states include `SUCCEEDED` and
  `FAILED`, and **drawing a final state as a dot collides head-on with the first hard rule above**
  (shapes must not be swapped: a final state is an axis-2 block). So the string is fixed at
  `PREPARING ─▶ STREAMING ─▶ COMMITTING` with **the trailing text "→ 终态待定"**, and axis 2 takes
  over once the run ends.

## 4. Data density

- Table row height **41px**, header row 39px, cell padding 14px, header on `--mute-bg` at **weight
  500**, hover row `#FAFAFA`. These four numbers are measured from x2doris, not invented. The
  emphasis weight **500** is also the criterion for V25.
- Numeric columns are **right-aligned and monospaced**; units follow in small `--mute` text and never
  enter the number itself.
- Tables always sit inside `.table-wrap` and **scroll horizontally on their own**; the page never
  scrolls sideways. **No outer border** — layering is the white card on grey ground alone. When there
  are too many columns to fit, **the action column pins to the right** (`.action-column`, matching the
  reference's `ant-table-cell-fix-right`).
- Key measurements go in a **key-value grid** (`auto-fit, minmax(150px, 1fr)`, with 1px gaps letting
  the ground show through as gridlines); field names at 11px `--mute`, values monospaced. The single
  most important number may go up to 17px.
- **No dashboard cards, no large donuts, no decorative icons.** What is being bought here is
  operability, not looks.

## 5. Business-value container

The `column` / `value` in failure details are **real business values from the source database**. They
have exactly one home in the UI:

- **A standalone alert box** (`--warn` border and ground), visually separate from ordinary fields,
  never mixed into a key-value grid or a table cell.
- **Masked by default** (`filter: blur` plus non-selectable); revealed only on clicking "显示".
- The box states plainly: **"显示即把源库真实值送进这台浏览器"**.

This is a **design placement, not a security mechanism**. The exposure is priced in `CONTEXT.md`
under **Known gaps**, item 1 (reachability equals credential privilege); this file does not reopen it.

**Datasources** (connection string, user, password) are first-class entities occupying a nav item with
a full management screen. **The password's presentation boundary is tight**: the interface never reads
a password back (not even the ciphertext), the password field is always empty in edit mode, and a
neutral badge beside it reads "已设置 · 留空 = 不改". **What is not drawn is authentication**, not the
connection configuration itself.

## 6. Dark theme: **not shipping**

V1 **ships light only**, with no dark theme and no theme switch. Three reasons:

1. The audience is ops and developers in an office; the reference itself is a light layout, and dark
   is not this group's default expectation.
2. Two themes are an **ongoing maintenance cost** — every new prototype needs checking twice.
3. What is being bought here is operability, not an appearance option.

**This is a positive answer, not something left to an implementation-time default.** Shipping dark
requires reopening a ticket and editing this section.
Note that "light only" does not conflict with the **dark left nav** — the reference's light layout has
a dark `#001529` sider.

## 7. Component inventory

Already formed in the prototype and directly reusable:

| Component | Key points |
|---|---|
| **App shell** | Left nav `--sider-w` **256px, dark `--sider-bg` `#001529`** (selected item filled `--brand`, 10px rounded block) + top bar `--topbar-h` **50px** (collapse trigger at far left + breadcrumb + environment pill at right) + grey content area |
| **Collapsed sider `.app-shell.is-collapsed`** | 256px ⇄ **48px**, trigger at the far left of the top bar (`menu-fold ⇄ menu-unfold`); collapsed shows **icons only, centred** — the reference does not adjust `padding-left` here, so its selected blue block is sliced into a vertical strip, which is its rendering defect and is **explicitly not copied**. Semantics carried by `title`; the collapsed state is stored in `localStorage` |
| **Card `.card`** | White ground + 4px radius + `--card-pad` padding, **no border and no shadow** (x2doris measures `border: none / box-shadow: none`); layering is the white card on grey ground alone |
| **In-card title block + toolbar `.table-title-row`** | On the left, a `--title-bg` grey title block (`--weight-em` 500) plus a small count; on the right, tool icons (refresh / row density / column settings) + primary button + bulk button. **The bulk button is disabled when nothing is selected** |
| **Filter strip `.filter-card`** | Controls at `--ctl-h` **32px** + primary button + ghost button, **standing as its own white card** above the table card |
| **Checkbox column `.check-column`** | The header is select-all, and it **selects the current page only** — selecting across pages would let someone act on rows they cannot see |
| **Progress cell `.progress`** | A thin bar + **one integer percentage**, floored, no decimals, no row counts appended. Three empty states all render `—`: never run, the pre-run count failed (`title` says so), and no denominator |
| **Pagination `.list-pagination`** | `共 N 条` + **page buttons** (current page filled `--brand`) + previous / next + **page-size dropdown** (20 / 50 / 100), at the bottom right inside the table card. Still **client-side paging that does not pretend to be server-side**; the whole strip is hidden when the total fits on one page |
| **Detail drawer `.drawer`** | 760px on the right, holding everything about a task's **most recent** run: plain-language conclusion bar + axis-2 block + axis-3 tag, row-count reconciliation, per-stage timings (with "pre-run count" as its own entry), the task definition, both ids, and the source SQL. **Re-run** sits at the bottom |
| **Buttons** | Three tiers — primary (solid `--brand`) / ghost (white with a `--line-strong` border) / link (`--brand` text) — with a 2px `--brand` focus ring |
| **Key-value grid `.kv`** | See §4 |
| **Data table `.data-grid`** | See §4. Inline actions are **all icons**, identified by `title`; two dividers split them into "run" ｜ "view / edit / rename" ｜ "delete", with delete alone tinted red |
| **Phase string `.phaseline`** | Axis 1 |
| **Final-state block `.term`** | Axis 2 |
| **Error code tag `.code`** | Axis 3 |
| **Plain-language conclusion bar `.plain`** | A 3px `--crit` rule on the left + `--crit-bg` ground, max 68ch, placed first on a failure page |
| **Business-value alert box `.sensitive`** | See §5 |
| **SQL placeholder `.sql .ph`** | **A blank for a person to fill in** inside a code block: `--warn` dashed border + `--warn-bg` ground, such as `<目标表名>` in the target DDL. It is **a hint, not an error** — `--warn` is chosen for the "not settled yet" tone; it is outside the three axes and **must not** be used to mark errors. Its premise is **never blocking someone from looking first**: a statement with placeholders is still handed over whole |
| **Highlighted SQL input `.sql-text-input` + `.sql-highlight`** | **The only widget with syntax highlighting**, used in exactly two places: the custom SQL editor (`.source-sql-editor`, which also carries a "format" button that **touches whitespace only and changes not one character**) and the filter clause's WHERE textbox (`.where-clause-editor`, which has no format button — a bare predicate has no clauses to lay out). Both are a transparent-text `textarea` over a coloured `<pre>`, **the two layers sharing a box**; only the starting height differs. It uses four tokens — `--sql-keyword` / `--sql-string` / `--sql-number` / `--sql-quoted` — which **carry no state semantics**, are unrelated to the three axes, and **must not** be used elsewhere; comments and punctuation fall back to `--mute` / `--dim`. Ligatures are off in both layers (`>=` must never render as `≥`) |

**Reserved placements** (the information architecture is decided elsewhere; the visual language must
cover it):

- **Run history**: no screen of its own — it is folded into the job center. The task list shows only
  the **most recent** run, with everything else in the detail drawer. The list's "run status" column
  is a **one-dimensional index**, not axis 2; axis 2 lives entirely in the drawer.
- **Two readings of progress**: the list's migration-progress column gives **an integer percentage**
  (the denominator being one `COUNT(*)` run before the transfer starts), using the "progress cell"
  above; the **run detail page uses an indeterminate bar**, which answers "is this stage still
  moving", not "how far along". **No new component for either.** The three "outcome unknown" cases
  produce a plain-language conclusion bar only, never an error code tag (§4).
- **SQL builder**: the precheck report has **only the mapping-precheck section** (the source performs
  no SQL shape precheck), rendered as a conclusion bar + data table.
- **Datasource screen** (see [ADR-0039](../adr/0039-v1-ui-increments.md) §1–§4):
  **app shell + card + data table + dialog, with no new components**; this section's inventory does
  not grow or shrink for it. The list gets no filter strip and no "connection status" column.
  Passwords use the neutral variant of the existing `.field-badge`; a successful connection test is
  one line of plain text, a failed one reuses the form error area and **echoes the driver's error
  verbatim without an error code tag** (error code tags belong to the protocol's closed set, and the
  metadata surface is not part of it).
  There is also a **"Target Agent"** column: MySQL rows show the agent name, followed by an existing
  `.state` tag when the bound agent is offline or its identity does not match; the cell is empty on
  Oracle rows. The "no connection status column" rule is **unchanged** — it governs **business
  databases**.
- **Target Agent screen** (see [ADR-0044](../adr/0044-target-agent-registry.md) §6):
  again **app shell + card + data table + dialog, with not one new component**; this section's
  inventory does not change for it. The status column reuses the existing solid `.state` tag
  (online `is-succeeded` / offline `is-unknown` / identity mismatch `is-failed`).
  **It has a status column, and that is an explicit exception to the rule above.** That rule guards
  against the cost of "background-polling every **business database**" and against the lie of a stale
  green dot. Probing an agent is one `GET /v1/agent/info` against our own process — it touches no
  business database and consumes no database connection — and "is it alive right now?" is the entire
  reason this screen exists.

## 8. Accessibility and degradation

- Semantics **never rest on colour alone**: each axis has its own shape, and final states and error
  codes always carry text.
- Focus is visible: every interactive element gets a 2px `--brand` ring on `:focus-visible`.
- Respect `prefers-reduced-motion`.
- The three axes stay distinguishable in black-and-white print or photocopy (solid block vs outline
  block vs dashed tag vs solid tag).
  **This rule is measurable rather than qualitative**: with the whole page under
  `filter: grayscale(1)`, the two axis-2 blocks must differ by **≥ 25/255 (≈10%) in median luminance
  inside the block**.
  It became a number because it genuinely failed in its qualitative form: an `--ok-bg` ground differed
  by only 8/255 (3.1%), and two walkthroughs in a row scored it "partially met" rather than FAIL,
  because "they are still distinguishable by shape" is always true (the border and text are there, so
  of course the blocks are distinguishable). **A qualitative criterion cannot catch this regression**:
  it asks "are they distinguishable", while the promise says "distinguishable by solid vs outline".
  The current `--ok-fill` **measures 227** against a white 255, a difference of **28 (11.0%)** — the
  browser's `filter: grayscale(1)` uses Rec.709 coefficients, 3 higher than the 224 estimated offline
  with ITU-601. **The walkthrough measurement governs**; the criterion measures the pixels actually on
  screen.
  The walkthrough **must record the measured number** and does not accept "looks distinguishable";
  the sampling method is in `m2-visual-walkthrough.md` V5 (full-page greyscale screenshot, then median
  luminance inside the block).

## 9. Known costs (accepted along with this choice)

- **A familiar paradigm is hard to make striking**: this interface will not turn heads. What it buys
  is zero learning cost.
- **Moderately loose density**: about 40% fewer rows per screen than a very high-density alternative,
  so reading long histories takes more scrolling.
