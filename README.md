# legacy

A local-first, self-hosted life-story vault. One person runs this on their own
machine over months or years to record their life story as plain Markdown and
media files, then seals the result so a threshold of trusted keyholders can
unlock it after their death. The unlocked archive can be read as plain files,
or loaded by an LLM to answer questions in the subject's own voice.

See [`ARCHIVE FORMAT`](#archive-format) below for the on-disk layout, and
[`docs`](#build-status) for what's implemented so far.

## Non-negotiable design principles

1. **Plain files on disk are the source of truth.** Markdown with YAML
   frontmatter for stories, original binaries for media. SQLite is only ever
   a rebuildable search index — delete it and `legacy timeline build` brings
   it back.
2. **No cryptography is implemented here.** Encryption is done by shelling
   out to [`age`](https://age-encryption.org), and key splitting by shelling
   out to the `shamir-mnemonic` (SLIP-0039) library.
3. **No lock-in.** No custom container format, no pickled Python, no
   vendor-specific codecs.
4. **Offline by default.** All LLM and speech features are opt-in per
   invocation. Recording, organizing, sealing, and unsealing the archive
   require no network access at all.
5. **The archive explains itself.** Every archive root has an unencrypted
   `README.txt` describing the layout, formats, encryption scheme, and the
   exact commands needed to reconstruct the key and decrypt — written for a
   reader in 30 years who has never heard of this tool.

## Installing

```bash
uv venv
uv pip install -e ".[dev]"
```

Requires `age` and `age-keygen` on `PATH` for anything encryption-related,
and `par2` for recovery-data generation on sealed archives. Both are
optional until you run `legacy seal` / `legacy verify`.

## Quick start

```bash
legacy init ./my-archive --subject "Jane Doe" --threshold 3 --shares 5
legacy story add --archive ./my-archive --title "Starting university" --date 1994-09-15 \
    --tags education,leaving-home --body "I packed one suitcase and took the train."
legacy story list --archive ./my-archive
```

## Archive format

```
my-archive/
  README.txt                  # unencrypted, always. Explains everything below.
  archive.yaml                # subject name, schema version, per-tier encryption config
  MANIFEST.sha256              # sha256sum-compatible checksum list
  timeline/
    1978/
      1978-03-anecdote-first-bicycle.md
    1994/
      1994-09-15-started-university.md
    undated/
      dad-and-the-fishing-trip.md
  people/
    mary-oconnor.md
  places/
    celbridge.md
  media/
    1994/
      1994-09-15-graduation-001.jpg
      1994-09-15-graduation-001.jpg.yaml   # sidecar metadata
  interviews/
    2026-08-14-childhood-session-01.md
    2026-08-14-childhood-session-01.wav
  sealed/
    family.tar.age               # created by `legacy seal`
    family.tar.age.par2
    executor-only.tar.age
    executor-only.tar.age.par2
```

### Story file format

```markdown
---
id: 1994-09-15-started-university
title: Starting university
date: 1994-09-15          # ISO 8601; may be YYYY, YYYY-MM, or YYYYs (decade) for fuzzy dates
date_precision: day       # day | month | year | decade | unknown
people: [mary-oconnor]
places: [dublin]
tags: [education, leaving-home]
media: [media/1994/1994-09-15-graduation-001.jpg]
source: interview:2026-08-14-childhood-session-01
recorded_at: 2026-08-14T19:22:31Z
tags_generated_by: null   # null if human, else model id -- never blurred
visibility: family        # public | family | executor-only
---

Free prose. This is the story as the subject told it, lightly cleaned up at most.
```

Rules the code enforces:
- Fuzzy dates are first-class: a story's date may be known only to the year,
  month, or decade, or not at all. The program never invents precision.
- The `id` is derived once from the date prefix (if any) and a slug of the
  title, and used as the filename; it also determines which `timeline/<year>/`
  bucket (or `timeline/undated/`) the file lives in.
- LLM-generated tags are written to frontmatter but always flagged via
  `tags_generated_by`; the human's own words in the body are never rewritten
  by a model.
- `visibility` gates which encrypted tier (if any) a story ends up in when
  the archive is sealed, and what the replica is allowed to discuss.

### archive.yaml

```yaml
schema_version: 1
subject:
  name: Jane Doe
created_at: 2026-08-14T00:00:00Z
tiers:
  family:
    threshold: 3
    shares: 5
    identity_sha256: null       # filled in by `legacy seal`
    plaintext_escrow: false
  executor-only:
    threshold: 3
    shares: 5
    identity_sha256: null
    plaintext_escrow: false
keyholders: []
replica:
  sunset: null
```

`legacy init` seeds both tiers with the same threshold/shares given on the
command line; edit `archive.yaml` by hand before sealing if you want, e.g.,
a looser 2-of-4 for `family` and a stricter 3-of-5 for `executor-only` — it
is plain YAML, no command required to change it.

### Encryption design

Stories marked `visibility: public` are never encrypted — they stay as
plain files in `timeline/` permanently, since they're intended to be
shared with anyone. `legacy seal` builds one encrypted tier bundle each for
`family` and `executor-only`: a fresh `age` identity is generated per tier,
used to encrypt that tier's files into `sealed/<tier>.tar.age`, split into
SLIP-0039 mnemonic shares at the tier's configured threshold, printed once
to the terminal, and then discarded — the archive never stores the key
itself, only the SHA-256 of it (to verify a reconstructed key before
concluding the archive is corrupt). `par2` recovery data is generated
alongside each sealed blob. A `--plaintext-escrow` flag is available per
seal run for cases where losing the family's access is a bigger risk than
the confidentiality it buys.

## Build status

- [x] Step 1: archive format, `init`, `story add`/`list`
- [ ] Step 2: `timeline build`, SQLite FTS index, `verify`, generated README.txt
- [ ] Step 3: `media ingest`
- [ ] Step 4: interview subsystem (text mode)
- [ ] Step 5: `seal`/`unseal`
- [ ] Step 6: REST API
- [ ] Step 7: LLM tagging, voice mode, replica (`ask`)
