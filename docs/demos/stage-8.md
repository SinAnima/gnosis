# Stage 8 demo

A scan of `fixtures/enterprise-mixed` plus a scripted catalog shows the new edges. No live Postgres is required. The commands below are the TUI queries. Headless scan output is unchanged apart from a name that matches more than one entity, which `explain` lists instead of hiding.

## What to look at

1. A document link whose href is a relative path that matches one file. `explain` on the source shows `hyperlinks_to` with resolution `exact`. The evidence span is that source file.
2. `office/book.xlsx` has `declares_formula` from the worksheet to the formula and `formula_references` from the formula to an in-workbook range. The formula text is stored. The value is not calculated.
3. `office/note.pdf` already has `contains` edges for its pages and bookmarks. That edge is unchanged.
4. A catalog foreign key whose target relation was discovered is `foreign_key_to` with resolution `exact`. A key whose target was not discovered stays on the edge with resolution `unresolved`. `why` on that relationship prints the target id and `target entity not in store`.
5. `cpp/rule.cpp` includes `rule.hpp`. The `includes` edge points at the header's module when that path is the only match.
6. Two sections named Overview. `explain Overview` prints both ids and both source paths, and it does not open a neighborhood.

An href that is only the word `overview` stays unresolved while `docs/overview.md` is in the scan. An `https://` Office hyperlink stays unresolved and does not create a web object. Two files that share the same relative suffix are `ambiguous`, and the edge lists both paths.

## Limits

The pass does not read table rows, calculate a formula, or invent a business rule. `defines` and `belongs_to` are named in the vocabulary and this stage does not emit them. A placeholder entity, marked `placeholder=true`, is not a document, workbook, relation, or module.
