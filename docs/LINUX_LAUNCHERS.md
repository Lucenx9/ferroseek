# Linux launcher repository research

Reviewed on 2026-11-01 against upstream READMEs. These are observations about
features and presentation, not comparative performance measurements.

## Walker

[Walker](https://github.com/abenz1267/walker) leads with an application-launcher
description and a screenshot. Its README lists desktop applications,
calculations, files, commands, web search, clipboard history and other Elephant
providers, then covers installation, dependencies, usage, configuration and
themes. It explicitly requires Elephant and the relevant providers.

Takeaway for FerroSeek: explain the daemon dependency in the installation path
and show concrete tasks before implementation details. A screenshot should show
the actual launcher rather than a mockup.

## Albert

[Albert](https://github.com/albertlauncher/albert) describes itself briefly as a
plugin-based, desktop-agnostic C++/Qt keyboard launcher. Its short README sends
users to the [project site](https://albertlauncher.github.io/) for installation,
basics, extensions and a gallery.

Takeaway for FerroSeek: keep the repository introduction brief and move engine
research and measurements into linked documentation. A separate website is not
required to make the README usable.

## Rofi

[Rofi](https://github.com/davatorium/rofi) names its main tasks immediately:
window switching, application launching and dmenu-style selection. It documents
matching, keyboard navigation, history, built-in modes, themes, installation and
Wayland-specific limitations. It distinguishes versions of its documentation.

Takeaway for FerroSeek: list actual keyboard controls, state compositor limits,
and distinguish available features from planned ones. Do not suggest that file
search also provides window switching or arbitrary command execution.

## Anyrun

[Anyrun](https://github.com/anyrun-org/anyrun) opens with its Wayland focus and a
screenshot. It documents GTK styling, asynchronous plugin execution,
installation dependencies, configuration and individual plugins for tasks such
as applications, calculations and file search. The README also states its
anyrun-provider dependency.

Takeaway for FerroSeek: make platform requirements and customization limits
visible. Themes and window configuration deserve priority before presenting the
launcher as ready for general daily use.

## Applied to FerroSeek

The README now follows the user journey: identity, implemented tasks,
requirements, installation, starting and binding a shortcut, keyboard controls,
current limits, then optional engine/agent tools. Benchmark and Baloo research
remain linked technical background. Noctalia and KRunner are removed from the
product, installer and distribution options; the launcher talks to a FerroSeek
file-search interface.

FerroSeek's current focus is applications, indexed file discovery and
calculations, with a Rust UI rendered by tiny-skia. The other projects' broader
feature lists are useful context, not a commitment to implement every provider.
Appearance settings and real compositor validation remain in the roadmap.
