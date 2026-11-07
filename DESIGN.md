# FerroSeek design guide

FerroSeek is a precise, quiet Linux launcher. Its defining experience is
**search → recognize → act → return to work**. Typography, spacing and feedback
should make that sequence easy to repeat with either a keyboard or a pointer.

This is the design contract for agents and contributors. Apply it to launcher
UI, interactions, user-facing copy, screenshots and product documentation.
Read it at the start of each task, as required by [AGENTS.md](AGENTS.md).
For engine-only work, preserve the product constraints without adding UI work.

The baseline below describes the current implementation. Rules for new work
are requirements, not claims that every existing view has been validated.
[LAUNCHER.md](docs/LAUNCHER.md) owns supported controls and desktop limitations;
[ROADMAP.md](docs/ROADMAP.md) owns feature priorities. Keep those documents and
this guide aligned when behavior changes. Resolve discrepancies explicitly;
do not weaken recovery, privacy or keyboard guarantees to match a mockup.

## Direction and priorities

1. **Trust before decoration.** Correct targets, reliable focus, readable results
   and recoverable failures take precedence over visual effects.
2. **The task has the strongest hierarchy.** Give the query and selected result
   prominence. Navigation, counts and hints should remain legible but quieter.
3. **Density comes from structure.** Align repeated fields, group related controls
   and remove redundant labels before reducing text size or hit areas.
4. **Reveal complexity in context.** Keep primary actions direct; place secondary
   commands in the searchable action panel and detailed information on request.
   Do not hide an essential action behind an unexplained icon or hover.
5. **One product, one vocabulary.** Reuse the same field, row, selection, footer
   and settings patterns across views. A new feature should feel familiar.
6. **Polish serves continuity.** Preserve the user's query, target and return path
   through delayed responses and secondary views. Motion must never delay input.

The visual identity is a compact neutral surface, readable two-line results,
an inset selection with a narrow accent bar, and explicit keyboard hints.
Iron is the default palette; Slate and Forest are equally supported alternatives.
Dark presentation is a reference direction, not permission to remove Light or
System mode. Use original compositions and platform conventions; do not import
another product's branding, assets, proprietary code or interface wholesale.

## Foundations

### Color and material

Use the semantic roles in
[appearance.rs](crates/launcher/src/appearance.rs), not per-view hex values.
The current Iron seeds are:

| Role | Dark | Light |
|---|---|---|
| Surface | `#17181C` | `#F4F2EE` |
| Primary text | `#ECEAE6` | `#25272C` |
| Accent | `#E0703A` | `#99421E` |

These are FerroSeek's existing values, not a new palette to apply over the
other themes. Derive all states through the shared roles:

| Semantic role | Use |
|---|---|
| `SURFACE` / theme background | Opaque content / material-aware outer shell |
| `RAISED` | Elevated controls and solid action panel |
| `TEXT` | Names, entered text, essential instructions |
| `DIM` | Paths, descriptions, counts and shortcut labels |
| `FAINT` | Quiet hover fills and separators; never body text |
| `BORDER` | Boundaries that help distinguish controls or surfaces |
| `ACCENT` / `SELECTED` | Focus, active markers / tinted selection fill |
| `ERROR` | Failures, accompanied by readable explanation |

Reserve accent for interaction and meaningful state. Use spacing before adding
another border. Avoid nested cards around ordinary result rows, decorative
badges, oversized shadows, glowing outlines and gradients behind task text.
Selection, hover and keyboard focus must remain distinguishable without relying
on hue alone: preserve the selection bar, active-filter underline and checked
settings choices.

Glass and Solid are independent of color mode. Preserve the existing opaque
fallback when blur or renderer transparency is unavailable, and keep action
menus solid. Glass must remain readable over bright and dark backgrounds;
do not lower surface opacity or secondary-text contrast for a screenshot.
The current Glass shell alpha is 0.88; `DIM` adapts to material and mode.
Use [material.rs](crates/launcher/src/material.rs) for capability handling.

### Typography and icons

Use iced's default font for UI text, with regular weight for content and
semibold for short section labels or primary actions. This follows the existing
font resolution; it does not guarantee a particular family on every desktop.
Do not bundle a web font or require a network font for visual similarity.
Use monospace for code or text previews where it helps reading, not as a
decorative label style.

Current sizes in [ui.rs](crates/launcher/src/ui.rs) are logical pixels:

| Purpose | Baseline |
|---|---|
| Main query and page title | 20; scoped query uses 15 |
| Result title | 15 |
| Action label | 14 |
| Supporting explanation | 13 |
| Result subtitle, navigation and footer | 12 |
| Shortcut badge and result type | 11 |

Keep main content left aligned. Use sentence case and a small number of weights.
Do not make labels uppercase, widen letter spacing or shrink essential text to
force a layout to fit. Preserve font fallback and test wide Unicode glyphs.
Measure labels with the rendered font through `label_for_width`; character
counts are not a layout budget. Preserve distinguishing filename endings and
path context when truncating, with full values available through details/copy.

Result icons occupy 28 logical pixels and come from the desktop icon lookup in
[icons.rs](crates/launcher/src/icons.rs). Keep alignment when an icon is absent.
Icons support names rather than replacing them; avoid decorative emoji or an
unrelated icon library. Pair unfamiliar controls with a visible label.

### Geometry and spacing

Use [layout.rs](crates/launcher/src/layout.rs) and existing view helpers as the
source of geometry. Values below are the baseline, not independent constants
to duplicate in each screen:

| Element | Current geometry, logical pixels |
|---|---|
| Width presets | Compact 640, Standard 720, Wide 880 |
| Optional preview | Adds 260 before output bounds are applied |
| Output edge allowance / shell padding | 16 / 8 |
| Header / footer | 72 / 34; narrow headers add 30 |
| Result slot | 48 high, including clickable padding |
| Result highlight | 44 high; inset 2 vertically and 4 horizontally |
| Result content gap / selection bar | 12 / 3 |
| Corner radii | Shell 14, actions panel 10, results 9, fields 6, keycaps 4 |

Prefer the existing 4/8/12/16 spacing rhythm for new groups; optical adjustments
belong in the owning component. Align icons, text starts and shortcut columns
across repeated rows. Rounded corners should express containment, not make
every element a pill. Preserve the full hit target around an inset highlight.

Fit the selected output and scale. Short windows show fewer rows; narrow
headers wrap controls before sacrificing the query or exit action. Test long
labels and the preview sidebar at Compact width. Respect saved width and
placement; never impose a new default solely to improve a promotional capture.

## View and interaction contracts

### Search and results

Keep the query on its own full-width row, filters and utility actions beneath,
results in the main region, and current action hints in the footer. Show the
result name first and useful distinguishing context second: parent path for
files, description for applications. Avoid repeated type badges or metadata
that does not help choose a result.

Enter performs the selected result's primary action. Ctrl+K reveals its actions;
Shift+F10, the Menu key, the footer button and context click provide the existing
alternative entry points. Show shortcuts beside the action they invoke, using
Linux key names. Keep the authoritative mapping in
[Keys](docs/LAUNCHER.md#keys), rather than inventing a separate scheme per view.

Single click selects, double click opens, and hover does not move the keyboard
selection. Asynchronous updates must preserve target identity; queued clicks
must never open a replacement row. A new query clears stale file results, and
late replies must not restore a superseded query. Keep text input responsive
while the file service is busy or unavailable.

### Actions and secondary views

Filter actions without rewriting the main query or changing the captured target.
Visible numbers identify the currently displayed actions, including after
filtering or scrolling. Short outputs show fewer commands so the target, filter
and return hint remain visible. Keep primary commands easy to find, and explain an
unavailable operation rather than leaving a plausible-looking dead control.

Escape unwinds the active context: clear the action filter before closing its
panel; return from a secondary view before dismissing the launcher. Preserve
the existing query-clearing and folder-return order documented in the launcher
guide. Restore focus so typing immediately after a transition reaches the
correct field. Preserve the first-run exception: Escape leaves initial folder
setup without writing configuration or starting the service. Never make the
pointer necessary to leave a view.

Details and previews should answer a selection question. Keep the selected file
and return context stable during loading, errors and cancellation. Retain the
explicit opt-in and resource bounds of the text preview. Do not turn the main
result list into a dashboard or add preview formats merely for visual richness.

### Settings and state feedback

Group settings by the user's decision, with consistent labels and a visible
current choice. Appearance previews immediately, but Save persists changes;
Back with unsaved edits requires explicit discard, and Escape at that prompt
keeps editing. Save failures retain the draft. Preserve the distinct save/apply
behavior of folder settings documented in [LAUNCHER.md](docs/LAUNCHER.md).

Design each affected state and its next step. The following are requirements
for new or changed flows; example wording must match the actual condition:

| State | Required treatment |
|---|---|
| Initial setup | Explain that file search needs indexed folders; expose folder selection. |
| Loading | Name the pending operation; retain usable controls and the safe return path. |
| No matches | Distinguish a completed empty search from loading; suggest changing query or scope. |
| Service unavailable | Explain file search availability; expose status/retry where supported. |
| Partial or bounded results | State the limitation; never imply an exhaustive list. |
| Success | Confirm the actual completed operation; keep copying and opening behavior distinct. |
| Failure | Explain what failed, retain useful context, and provide a supported recovery action. |
| Unsaved changes | Make Save, discard and continued editing explicit. |

Use direct English such as “Copy path,” “Show in folder” and “Appearance saved.”
Avoid generic “Something went wrong,” backend jargon in normal flows, or
success messages before completion. Technical details belong in relevant
diagnostics. Never hide a failure with a transient decorative toast.

## Motion and accessibility

Immediate state changes are the launcher baseline. Do not animate every result,
stagger search arrivals, move rows under the pointer or add ambient loops.
Keyboard navigation, focus changes and dispatch must not wait for an effect.

If a future transition needs motion to explain a change, prefer a short opacity
transition, roughly 100–160 ms, with no bounce or overshoot. This is a proposed
design range, not an implemented timing token or a performance measurement.
Ship nonessential motion only with a verified reduced-motion path; otherwise
keep the change immediate. This guide does not claim existing reduced-motion
settings or desktop accessibility support.

Maintain at least 4.5:1 contrast for small essential text against its actual
composited background. Preserve the existing palette contrast regression in
[ui.rs](crates/launcher/src/ui.rs), covering all palettes, both modes and
materials. Inspect focus, selection and error indicators separately; passing
text checks does not certify every control or assistive-technology workflow.
Use labels and shapes as well as color. Keep text selection and native editing
behavior intact. Validate keyboard access, focus visibility and scaling for
each changed flow, and report untested accessibility behavior honestly.

## Product presentation

The README and any future product page should demonstrate a real task with a
current FerroSeek capture and concise explanation. Use generous space around
the demonstration, a clear heading hierarchy and readable technical examples.
Keep installation, requirements and limitations easy to find. Show implemented
behavior; identify planned work through the roadmap.

Editorial layouts, restrained gradients or a brief demonstration transition may
support a future presentation page when they clarify the story. They are not
launcher UI defaults. Avoid scroll-dependent instructions, autoplay decoration,
generic feature-card grids and unsupported speed claims. Use synthetic data in
captures and record the renderer, scale and compositor; never publish real user
paths or present virtual validation as physical-desktop evidence.

## Applying the guide

Before editing, identify the user task, the affected entry and exit paths, and
the existing component that owns the behavior. Read the relevant component
guide from [AGENTS.md](AGENTS.md). Reuse shared styling in `ui.rs`, semantic
colors in `appearance.rs` and geometry in `layout.rs`; add a new abstraction
only for current callers. Fix a shared inconsistency at its owner.

Before completing a UI change:

- Check keyboard and pointer paths, immediate typing after transitions, the
  selected target, asynchronous updates, errors, cancellation and return focus.
- Inspect real before/after renders for layout changes, including long names,
  Light/Dark, affected palettes, Glass/Solid and constrained output sizes.
- Reproduce logic bugs with a focused test before fixing them. Run relevant
  checks and the isolated graphical harness from
  [Tests](docs/LAUNCHER.md#tests); keep real HOME, indexes and session buses untouched.
- Update affected controls, limitations and screenshots in place. If changing
  a baseline or introducing an intentional exception, explain the user benefit
  in the PR and update this guide in the same change.
- Report checks actually run and gaps that remain. Use
  [BENCHMARKS.md](docs/BENCHMARKS.md) for measured speed claims. Documentation-only
  work needs link, consistency and diff checks, not a new graphical run.

Agents must apply these rules during implementation and self-review. Do not
silently substitute another visual system, turn inspiration into new product
scope, or mark a design complete based only on a mockup.
