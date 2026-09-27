# Working on muz

These instructions apply to work in this repository and the maintainer's local
workspace. For the technical reading path and repository map, start with the
[documentation index](docs/README.md).

## Repository and project boundaries

This repository holds the engine, standard library, and independent examples.
Never add a piece's source, assets, or fixtures here. Keep generated media and
external samples out of git, and document chosen assets in the piece's project.

In this maintainer's workspace, pieces live in the sibling `muz-projects`
checkout, one directory per piece (`../muz-projects/<piece>/`). This is a personal
organization convention, not a requirement for other muz users. Public examples
must allow users to choose their own project locations.

Pieces remain freely editable music, not regression fixtures. Protect engine
behavior with small synthetic cases, and keep engine examples independent of
particular pieces.

## Required reading and friction reports

Before composing, revising a piece, or changing the engine, read the
[composer friction protocol and live log](docs/composer-friction.md).
Record encountered engine and language issues even when a workaround lets the
piece proceed. External services, model quality, skill tooling, and development
process reports do not belong in that log.

Commit new reports with the piece or task that exposed them. Remove each report
in the commit that resolves it, with relevant reference documentation and focused
verification. Do not maintain separate project friction logs or resolved lists.

## Resolving engine and language friction

Seek a general solution with the smallest necessary expansion of the
builtins/kernel. First consider existing language operations and exposed data.
If something is missing, prefer a general primitive that enables composers to
implement a family of solutions. Put specific policies and recipes in `std/`,
project source, or examples.

Follow the resolution guidance in the [friction protocol](docs/composer-friction.md#choosing-a-resolution).
This rule applies to all friction, including tag-derived automation.

## Verification budget

For routine rendering, encoding, exports, and file operations, treat a successful
command exit without errors as sufficient. Stop there unless the user requests
measurements or a concrete symptom needs investigation. Avoid automatic
encode/decode round trips, repeated full-file analysis, and verification-only
media copies. When investigating, use the smallest relevant check and clean up
any temporary media it creates.

For engine or language changes, use focused synthetic tests for the changed
behavior. Scale verification to the change; documentation edits need only diff
review. Historical production reports and critique checklists describe prior
work, not mandatory delivery gates. This policy also applies when maintaining
project delivery scripts.
