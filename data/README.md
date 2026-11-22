# Data

This directory is for local, git-ignored imports and test corpora that are not
part of the repository.

Expected layout when an importer exists:

```text
data/
  imported/        legacy dictionaries, layouts, and migrations (ignored)
  corpus/          licensed/synthetic test corpus (ignored unless redistributable)
  exported/        user-requested exports (ignored)
```

Rules:

- never commit user learning data or typed text;
- never commit proprietary legacy assets;
- record provenance and license for any redistributable corpus;
- encrypt sensitive exports;
- delete imported data when the user clears learning data.
