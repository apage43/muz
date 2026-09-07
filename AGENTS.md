# Working on muz

Before composing, revising a piece, or changing the engine, read
[the composer friction protocol and live log](docs/composer-friction.md).
Record encountered engine and language issues even when a workaround lets the
piece proceed. The friction log is for issues we can fix in muz's engine or
language, not external services, model quality, skill tooling or development
process reports.
Commit new reports with the piece or task that exposed them; remove each report
in the commit that resolves it, with relevant reference documentation and focused
verification. Do not maintain separate project friction logs or resolved lists.

Pieces remain freely editable music, not regression fixtures. Protect engine
behavior with small synthetic cases where needed. Keep generated media and
external samples out of git; document chosen assets in the project.

Pieces live in the sibling `muz-projects` checkout, one directory per piece
(`../muz-projects/<piece>/`). This repository is the engine alone: never add a
piece's source, assets or fixtures here, and keep engine examples independent of
them.

When resolving any friction item, seek a general solution with the smallest
necessary expansion of the builtins/kernel. First consider existing language
operations and exposed data; if something is missing, prefer a general primitive
that enables composers to implement a family of solutions. Put specific policies
and recipes in `std/`, project source, or examples. Follow the resolution guidance
in the [friction protocol](docs/composer-friction.md); this rule applies to all
friction, not just tag-derived automation.
