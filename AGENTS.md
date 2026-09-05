# Working on muz

Before composing, revising a piece, or changing the engine, read
[the composer friction protocol and live log](docs/composer-friction.md).
Record encountered friction even when a workaround lets the piece proceed.
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
