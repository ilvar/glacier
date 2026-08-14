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

# Regenerate timeline.md, the search index, MANIFEST.sha256, and README.txt
# after adding stories or editing archive.yaml:
legacy timeline build --archive ./my-archive

# Check the archive against MANIFEST.sha256 (and par2 recovery data once sealed):
legacy verify --archive ./my-archive

# Encrypt the family and executor-only tiers; prints shares to the terminal ONLY:
legacy seal --archive ./my-archive

# Reconstruct a key from enough shares and decrypt the matching tier:
legacy unseal --archive ./my-archive --share "..." --share "..."
```

## Archive format

```
my-archive/
  README.txt                  # unencrypted, always. Explains everything below.
  verify-archive.sh           # plain-bash integrity check, no `legacy` install required
  archive.yaml                # subject name, schema version, per-tier encryption config
  MANIFEST.sha256              # sha256sum-compatible checksum list
  timeline.md                  # generated chronological index (legacy timeline build)
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

### Interview subsystem

Question banks live in `src/legacy/templates/interviews/*.yaml`, plain YAML,
editable directly:

```yaml
name: childhood
description: Early years, family, home
questions:
  - id: earliest-memory
    prompt: What is the earliest thing you can remember?
    followups:
      - How old were you?
      - Who else was there?
    tags: [childhood, memory]
```

Ships ten banks: `childhood`, `family-and-origins`, `school`, `work`,
`love-and-partnership`, `parenthood`, `beliefs-and-values`, `turning-points`,
`advice-and-messages`, `objects-and-places`.

```bash
legacy interview start childhood --archive ./my-archive
legacy interview resume 2026-08-14-childhood-session-01 --archive ./my-archive
```

Each answer is written as a story file (`timeline/<year>/<session-id>-<question-id>.md`,
`source: interview:<session-id>`) **immediately** after you finish typing it —
before the session state file is updated — so a crash mid-session never loses
an already-given answer. During a session: finish an answer with a line
containing just `END`, type `SKIP` alone to move on without answering, or
`QUIT` to stop and resume later. The session itself is recorded at
`interviews/<session-id>.md`: which questions are answered/skipped, which
story each became, and a running transcript. `--voice` mode (record audio,
transcribe with Whisper, keep both) lands in step 7 — the question bank and
resumability work fully offline without it.

### Media sidecar format

Every file under `media/` has a same-named `.yaml` sidecar recording what the
program knows about it -- the original is never modified or transcoded:

```yaml
original_filename: IMG_0001.JPG
ingested_at: '2026-08-14T16:40:10Z'
sha256: 53d5e412...
size_bytes: 6699
mime_type: image/jpeg
date: '1994-09-15'
date_precision: day
date_source: manual        # exif | filesystem | manual | unknown
width: 3000
height: 2000
duration_seconds: null
visibility: family
caption: null               # free text, edit by hand
```

`legacy media ingest <dir> [--date-from-exif] [--date YYYY[-MM[-DD]]] [--visibility ...]`
copies every recognized media file from `<dir>` into `media/<year>/`, named
`<date>-<slug-of-original-name>-NNN.<ext>` (or `media/undated/...` if no date
is known), and writes its sidecar. `--date-from-exif` best-effort reads a
capture date via `ffprobe`; without it (or if nothing is found), the date is
left `unknown` until you either pass `--date` or hand-edit the sidecar YAML.

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

`legacy seal` never touches or deletes the plaintext archive — it is purely
additive. Your working copy stays exactly as it was; `sealed/<tier>.tar.age`
is a distributable snapshot for keyholders. `people/`, `places/`, and
`interviews/` (session transcripts and audio) are bundled into the
**family** tier only, not duplicated into `executor-only` — they're shared
reference material and raw interview sessions, which default to
family-level sensitivity.

`legacy unseal --share "..." --share "..."` reconstructs the identity from
however many shares you give it, matches its SHA-256 against `archive.yaml`
to figure out which tier it belongs to (no need to specify), and decrypts
that tier into `<archive>/unsealed/<tier>/`.

## REST API

`legacy serve` runs a FastAPI app that mirrors the CLI 1:1 (JSON in, JSON
out), bound to `127.0.0.1` by default. There is no authentication in v1 —
it's meant for a single local user (e.g. a future GUI) driving the same
machine, not a multi-tenant service. If you expose it beyond localhost,
put a reverse proxy with auth in front yourself. There's no server-side
session state beyond the interview session id, which is just the filename
of an already-persisted session file.

```bash
legacy serve --port 8000

curl -X POST localhost:8000/stories -H 'content-type: application/json' -d '{
  "archive": "./my-archive", "title": "Starting university", "date": "1994-09-15"
}'
curl "localhost:8000/stories?archive=./my-archive&year=1994"
```

Routes: `POST /init`, `POST /stories`, `GET /stories`, `POST /timeline/build`,
`GET /verify`, `POST /media/ingest`, `POST /interviews`,
`GET /interviews/{id}`, `POST /interviews/{id}/answer`,
`POST /interviews/{id}/skip`, `POST /seal`, `POST /unseal`. `POST /seal`'s
response body contains the shares — same "printed once, never stored"
rule as the CLI, just delivered over HTTP instead of the terminal.

## Build status

- [x] Step 1: archive format, `init`, `story add`/`list`
- [x] Step 2: `timeline build`, SQLite FTS index, `verify`, generated README.txt
- [x] Step 3: `media ingest`
- [x] Step 4: interview subsystem (text mode)
- [x] Step 5: `seal`/`unseal`
- [x] Step 6: REST API
- [ ] Step 7: LLM tagging, voice mode, replica (`ask`)
