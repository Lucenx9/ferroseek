# Baloo (KF 6.31) architecture analysis for FerroSeek

Research date: 2026-10-31. Scope: what Baloo actually does today, where it hurts, what the
KRunner D-Bus protocol requires, and what that means for FerroSeek (a Rust daemon for
real-time file search with a native KRunner D-Bus runner).

## 0. Sources and conventions

| Source | Revision | Local path |
|---|---|---|
| Baloo (`invent.kde.org/frameworks/baloo`) | tag `v6.31.0` (`26787af0`), the installed version (`baloo 6.31.0-2.1`) | `/tmp/ferroseek-research/baloo-v6.31.0` (worktree of `/tmp/ferroseek-research/baloo`) |
| Baloo master (for "what changed after 6.31") | `34c6c5a9` (2026-10-31) | `/tmp/ferroseek-research/baloo` |
| Baloo full history (blobless) | – | `/tmp/ferroseek-research/baloo-history` |
| KRunner (`frameworks/krunner`) | tag `v6.31.0` (`aed9a970`) | `/tmp/ferroseek-research/krunner` |
| plasma-workspace `runners/baloo` | tag `v6.8.2` (`30beb009`), installed 6.8.2 | `/tmp/ferroseek-research/plasma-workspace` |
| KFileMetaData | tag `v6.31.0` | `/tmp/ferroseek-research/kfilemetadata` |
| plocate | Debian source `1.1.25-1` (files fetched individually) | `/tmp/ferroseek-research/plocate-src` |
| Installed files | `/usr/share/dbus-1/interfaces/*`, `/usr/share/krunner/dbusplugins/*`, `/usr/lib/systemd/user/*` | – |

- A bare `src/...:N` path refers to the **Baloo v6.31.0** tree. Other repos are prefixed, e.g.
  `krunner:src/dbusrunner.cpp:30`, `plasma-workspace:runners/baloo/baloosearchrunner.cpp:72`.
- 50 commits separate v6.31.0 from master. They touch query/term code (`searchstore.cpp`,
  `transaction.cpp`, `termgenerator.cpp`, `extractor/app.cpp`, …). Files outside that diff have
  identical line numbers in both trees.
- I fetched every bug cited here through the bugs.kde.org REST API
  (`https://bugs.kde.org/rest/bug/<id>`) and read its title and status. I read the comment text for
  the bugs where I describe their content.
- **Unverified** marks claims I did not confirm against a primary source.

---

## 1. Filesystem scanning and discovery

### 1.1 Configuration (`baloofilerc`, `FileIndexerConfig`)
- The config file is `baloofilerc`, generated through KConfigXT (`src/lib/baloosettings.kcfg:8`). Defaults:
  - `[Basic Settings] Indexing-Enabled=true` (`src/lib/baloosettings.kcfg:10-13`)
  - `index hidden folders=false` (`:16-19`)
  - `only basic indexing=false`, so content indexing is on by default (`:24-27`)
  - `folders` defaults to `$HOME` (`:38-41`)
  - `exclude filters` defaults to `defaultExcludeFilterList()` (`:46-49`)
  - `exclude mimetypes` defaults to `defaultExcludeMimetypes()` (`:42-45`)
- **Default exclude filters** (`src/file/fileexcludefilters.cpp:12-100`, list version 9 at `:102`):
  - Temp, backup and build artifacts: `*~ *.part *.o *.la *.moc moc_*.cpp CMakeCache.txt build.ninja …`
  - Disk images: `*.img *.qcow2 *.vmdk *.vdi *.vhd(x)`
  - Bytecode: `*.class *.pyc *.qmlc *.jsc`
  - Databases and dumps: `*.db *.sql *.sql.gz`
  - Bioinformatics bulk data: `*.fastq *.fasta …`
- **Default folder excludes** (`:104-139`, version 5): `.git .svn .hg .bzr CVS _darcs CMakeFiles node_modules .npm .yarn __pycache__ .venv venv .terraform core-dumps lost+found .snapshots` and others.
- **Default excluded mimetypes are the source-code types** (`:143-197`, `:228-231`), e.g. `text/x-c++src`, `text/rust`, `application/json`, `application/xml`. These files are still indexed by **filename**. Only content extraction is skipped. The check happens in the extractor (`src/file/extractor/app.cpp:228-236`).
- Filter-list upgrades are additive. A newer default list is merged into the user's list. The code carries a TODO noting that a default the user deleted comes back (`src/file/fileindexerconfig.cpp:32-50`).
- **Hidden files:** a leading `.` is rejected unless `indexHidden` is set (`src/file/fileindexerconfig.cpp:197-203`). `shouldBeIndexed()` checks every path component after the include root against the filters (`:160-195`).
- **Batch size:** `m_maxUncomittedFiles(40)` is hard-coded (`src/file/fileindexerconfig.cpp:62`). It is the content-indexing batch size; see §5.

### 1.2 Mounts and removable media
- `buildFolderCache()` adds every Solid device that is not "usable" and has a mount path as an *excluded* folder, unless the user explicitly included that mount path (`src/file/fileindexerconfig.cpp:321-331`).
- `StorageDevices::Entry::isUsable()` treats the following as unusable (`src/file/storagedevices.cpp:137-173`):
  - removable or hot-pluggable drives
  - network shares
  - optical discs
  - **any `/etc/fstab` entry** (`parentUdi == "/org/kde/fstab"`)
  - inaccessible volumes
- KInotify subscribes to `IN_UNMOUNT` on every watch (`src/file/kinotify.cpp:119-120`) and drops the watch when it fires (`:445-455`).

### 1.3 Directory walking (`FilteredDirIterator`)
- The walker is an iterative DFS over `QDirIterator` with a stack (`src/file/filtereddiriterator.cpp:55-107`).
- Its filters are `QDir::NoDotAndDotDot | Readable | NoSymLinks | Hidden` (`:15`). **Symlinks are never followed or indexed**. Hidden entries are listed and then filtered by config (`:86-101`).
- File IDs come from `lstat` (`QT_LSTAT` in `src/engine/idutils.h:48`), so a symlink is never resolved even when one reaches the ID code.
- **Hard links** share an ID. The first-run indexer skips duplicates with a FIXME (`src/file/firstrunindexer.cpp:54-61`).

### 1.4 First run (`FirstRunIndexer`)
- It runs only when `~/.local/share/baloo/index` did not exist at startup (`src/file/main.cpp:59`).
- For each include folder, inside one RW transaction (`src/file/firstrunindexer.cpp:36-76`), it:
  1. walks the tree,
  2. guesses the mimetype by **extension only** (`QMimeDatabase::MatchExtension`, `:46`),
  3. runs a `BasicIndexingJob` (`stat`, filename terms, mimetype term, type terms, xattrs; `src/file/basicindexingjob.cpp:205-260`),
  4. commits every 20 000 documents (`:64-70`).
- The source admits that memory is unbounded until each commit: `// FIXME: This would consume too much memory` (`:73-74`).
- Files are marked for phase-2 content indexing (`MarkForContentIndexing`, `:33-34`; `src/file/basicindexingjob.cpp:244-247`).

### 1.5 Startup reconciliation (`UnindexedFileIterator` / `UnindexedFileIndexer`)
- This pass runs at every daemon start. The first run is the exception (`src/file/fileindexscheduler.cpp:92-95,177-186`).
- It walks every included tree again and, for each entry, does `stat` plus an LMDB lookup of the stored `mtime`/`ctime` (`src/file/unindexedfileiterator.cpp:70-124`).
- Timestamps are **32-bit seconds** (`src/engine/documenttimedb.h:28-38`).
- A directory is re-indexed only when its ctime changes. Its mtime is ignored (`src/file/unindexedfileiterator.cpp:96-108`).
- Changed or new files are re-added. Content is re-queued only when mtime changed (`src/file/unindexedfileindexer.cpp:30-70`). Commits again happen every 20 000 documents (`:61-67`).
- This full walk at every login is how Baloo repairs events lost while it was not running, or lost to queue overflow (see §2.2). Its cost is O(files) `stat` calls plus B-tree lookups per login.

### 1.6 Stale-entry cleanup (`IndexCleaner`)
- It only walks the **excluded folders**. For each one that has a document, it removes recursively the entries that no longer exist or should not be indexed (`src/file/indexcleaner.cpp:28-58`).
- It does **not** look for documents whose ID became unreachable, for example after a device-ID change (see §9).

---

## 2. inotify handling (`KInotify`, `FileWatch`)

### 2.1 Watch setup
- There is one inotify fd (`inotify_init`, not `inotify_init1`; `FD_CLOEXEC` is set afterwards; `src/file/kinotify.cpp:182-192`) and **one watch per directory**.
- Watches are installed by a DirsOnly `FilteredDirIterator` in batches of 1000 per event-loop turn (`src/file/kinotify.cpp:150-179`). That means a full tree walk at every start, in addition to the reconciliation walk in §1.5.
- The watched mask is `IN_MOVE | IN_DELETE | IN_DELETE_SELF | IN_CLOSE_WRITE | IN_CREATE | IN_ATTRIB | IN_MODIFY` with `IN_ONLYDIR` (`src/file/filewatch.cpp:62-71`). `IN_UNMOUNT` and `IN_EXCL_UNLINK` are always added (`src/file/kinotify.cpp:120`).
- Two path↔wd `QHash` maps hold full path byte strings, so memory is O(directories × path length) (`:78-80`).
- **New directories:** when `IN_CREATE|IN_ISDIR` arrives, the new subtree is walked immediately. A synthetic `created` event is emitted for each child, and a watch is added per subdirectory. This closes the race between `mkdir` and `add_watch` (`:277-290`, `:376-384`).

### 2.2 Overflow (`IN_Q_OVERFLOW`)
- Since commit `772b3ed1` (2026-09-08, included in v6.31.0), the parser:
  1. skips the overflow marker,
  2. keeps the valid events already read,
  3. emits `eventQueueOverflowed()` (`src/file/kinotify.cpp:326-335`, signal declared at `src/file/kinotify.h:96-99`).
- **Nothing connects to that signal.** `FileWatch` connects `moved/deleted/created/modified/closedWrite/attributeChanged/watchUserLimitReached/installedWatches` only (`src/file/filewatch.cpp:47-54`). A grep of `src/` shows no other consumer.
- So in 6.31 an overflow is logged and the dropped events are lost until the next startup reconciliation or a manual `balooctl check`.
- Bug 437754 (CONFIRMED) describes exactly this: creating 50 000 files "hits the inotify 'event limit' and baloo doesn't get told of all the new files. Run `balooctl check`…" — https://bugs.kde.org/show_bug.cgi?id=437754
- Kernel semantics: "Events in excess of this limit are dropped, but an IN_Q_OVERFLOW event is always generated", with the limit set by `/proc/sys/fs/inotify/max_queued_events` (inotify(7)). It is `16384` on this machine.
- Reading: `ioctl(FIONREAD)`, then `malloc(avail)` and `read` per wakeup (`src/file/kinotify.cpp:292-312`).

### 2.3 Rename pairing via cookies
- `IN_MOVED_FROM` stores `{deadline = now + 1 s, path, flags}` keyed by `event->cookie` (`src/file/kinotify.cpp:408-414`). All events in one read buffer share one deadline (`:318`, `:410-412`).
- A matching `IN_MOVED_TO` rewrites the wd↔path maps for directories and emits `moved(old, new)` (`:415-432`).
- `IN_MOVED_TO` with no cookie (moved in from an unwatched place) becomes `created` plus a subtree walk (`:433-439`).
- A cookie that expires (1 s timer, `:205-209`, `:472-491`) becomes `deleted`. Its watches are removed (moved out of the watched tree).
- `IN_MOVE_SELF` whose parent is not watched only produces the log line `"THIS CASE MAY NOT BE HANDLED PROPERLY!"` (`:401-407`).
- In the DB, a move is cheap. `MetadataMover::updateMetadata` rewrites only the moved node's `(parentId, name)` and its filename terms (`src/file/metadatamover.cpp:76-123`; `src/engine/documenturldb.cpp:78-103`). Paths are stored as parent pointers, so **renaming a directory is O(1)** regardless of subtree size.

### 2.4 Watch limits (`ENOSPC`)
- On `ENOSPC` from `inotify_add_watch`, KInotify sets `userLimitReachedSignaled` and emits `watchUserLimitReached` (`src/file/kinotify.cpp:129-138`).
- `FileWatch` reacts by **deleting the whole KInotify object** and logs "File changes will be ignored" (`src/file/filewatch.cpp:153-166`). The comment reads: "we do it the brutal way for now hoping with new kernels and defaults this will never happen".
- Result: one directory over the limit means **no live updates at all** for the rest of the session.
- Bug 381622 (duplicate of 367480) is a user report showing "Failed to create watch … No space left on device" with `max_user_watches=8192`: https://bugs.kde.org/show_bug.cgi?id=381622
- On this machine `max_user_watches=524288` and `max_user_instances=1024`.

### 2.5 Event coalescing and debounce (`PendingFileQueue`)
- Events are merged per path in a 10 ms cache (`src/file/pendingfilequeue.cpp:20-24`, `:47-80`).
- Deleting a directory purges queued children (`:51-63`).
- Re-modified files back off exponentially from 5 s to 60 s, tracked for 120 s (`:26-40`, `:89-101`).
- `IN_CLOSE_WRITE` is only honoured if the file or its directory was modified recently (`src/file/filewatch.cpp:124-141`). The code compares `QDateTime::secsTo()` (seconds) against `1000 * 60`, so the effective window is 60 000 s (≈16.7 h), not the "last minute" the comment claims. This is a minor upstream bug, observed directly in code.

---

## 3. Persistence: LMDB

### 3.1 Environment
- The index lives in a single file, `$XDG_DATA_HOME/baloo/index` (plus `index-lock`). `BALOO_DB_PATH` overrides the location (`src/engine/global.cpp:16-25`).
- Flags are `MDB_NOSUBDIR | MDB_NOMEMINIT` (`src/engine/database.cpp:131`). There is no `MDB_NOSYNC`/`MDB_NOMETASYNC`/`MDB_WRITEMAP`, so **every commit is fully fsync'd** (LMDB defaults; flags in `/usr/include/lmdb.h:289-295`).
- Map size is **256 GiB** of virtual address space on 64-bit (`src/engine/database.cpp:112-128`). `maxdbs=12` (`:110`).
- On creation, Baloo sets `FS_NOCOW_FL` (chattr +C) on the directory so Btrfs does not CoW the mmap'd file (`src/engine/database.cpp:96-98`; `src/engine/fsutils.cpp:21-69`).
- `mdb_reader_check` runs at open (`src/engine/database.cpp:148-154`).
- Baloo never calls `mdb_env_copy*`/`MDB_CP_COMPACT` (grep of `src/`). An LMDB file never shrinks; free pages are only reused. The only way to compact is `purge` and rebuild. **Unverified**: that free-page reuse alone explains the growth reports in §9.

### 3.2 Named databases (real names, from `src/engine/database.cpp:168-184,207-223`)
| DBI name | Flags (file:line) | Key → Value |
|---|---|---|
| `postingdb` | none (`src/engine/postingdb.cpp:31`) | term bytes → sorted `u64` docId array, **raw 8 bytes each, no compression** (`src/codecs/postingcodec.cpp:12-27`) |
| `positiondb` | none (`src/engine/positiondb.cpp:32`) | term → per-doc position lists (phrase queries) |
| `docterms` | INTEGERKEY (`src/engine/documentdb.cpp:29`) | docId → list of content and property terms (needed for deletion) |
| `docfilenameterms` | INTEGERKEY | docId → filename terms |
| `docxatrrterms` (sic, misspelled in code) | INTEGERKEY | docId → xattr terms (tags, rating, comment) |
| `idtree` | INTEGERKEY (`src/engine/idtreedb.cpp:27`) | parentId → sorted child ids |
| `idfilename` | INTEGERKEY (`src/engine/idfilenamedb.cpp:28`) | docId → `{parentId, name}` (`src/engine/idfilenamedb.h:27-34`) |
| `documenttimedb` | INTEGERKEY (`src/engine/documenttimedb.cpp:28`) | docId → `{u32 mtime, u32 ctime}` |
| `documentdatadb` | INTEGERKEY (`src/engine/documentdatadb.cpp:27`) | docId → compact JSON of extracted properties (`src/file/extractor/result.cpp:105-108`) |
| `indexingleveldb` (the "content indexing DB") | INTEGERKEY (`src/engine/documentiddb.cpp:28`) | set of docIds pending content extraction ("phase one") |
| `failediddb` | INTEGERKEY | set of docIds whose extraction failed |
| `mtimedb` | INTEGERKEY, DUPSORT, DUPFIXED, INTEGERDUP (`src/engine/mtimedb.cpp:30`) | mtime → docIds (date-range queries) |

Notes:
- The task's names `contentindexingdb` and `failedids` do not exist as written. The real names are `indexingleveldb` and `failediddb`.
- Full paths are rebuilt by walking `idfilename` parent links. The depth limit is 512, "to deal with possibly corrupted DBs" (`src/engine/documenturldb.cpp:162-195`). Each result path therefore costs one B-tree lookup per path component.

### 3.3 Transactions and write batching
- LMDB allows a single writer and MVCC readers. **Two processes write the same environment**: `baloo_file` (basic indexing, moves, deletes) and `baloo_file_extractor` (content). LMDB's writer lock serializes them.
- `WriteTransaction` buffers per-term add/remove operations in a `QHash<term, QVector<Operation>>` (`src/engine/writetransaction.cpp:90-109`). At commit it does a **read-modify-write of the whole posting list** for every touched term: get, `sortedIdInsert`/remove, put. It does the same for the full position list (`src/engine/writetransaction.cpp:293-325`).
- Two consequences:
  - Adding one document containing a common term rewrites that term's entire posting list. With raw `u64` postings, a term in 500 k docs is a 4 MB value rewritten on each commit that touches it. Common terms include mimetype terms like `Mapplication`/`Mpdf`, type terms like `T5`, and common words.
  - Deleting many files rewrites many huge lists. Bug 442453 (CONFIRMED) reports 20-100 MB/s of writes and index growth from 5.95 to 6.42 GiB in a minute while *deleting* files. A 2026-07 comment attaches a 6.28.0 reproduction titled "466 GB writes and 8.39 GB memory peak". https://bugs.kde.org/show_bug.cgi?id=442453
- Batch sizes:
  - First run and reconciliation commit every 20 000 docs.
  - Content extraction fetches 40 ids per batch (`src/file/fileindexerconfig.cpp:62`) and commits at the end of the batch or **at least every 2 s** (`src/file/extractor/app.cpp:117-122`).
- Content-indexing order follows the integer key order of `indexingleveldb`, i.e. docId order (`src/engine/documentiddb.cpp:101-120`). Recency and user relevance play no part.

### 3.4 Document IDs (device + inode)
- `id = (u32 st_dev) | (u32 st_ino) << 32`. It is packed via `memcpy` of `[devId, inode]` and the code says "We're losing 32 bits of info" (`src/engine/idutils.h:22-43`).
- Since commit `c735faf5` (2023-09-07, "Use the FSID as the device identifier where possible", BUG 402154), `st_dev` is replaced by `statvfs().f_fsid` folded to 32 bits whenever `f_fsid != 0` (`src/engine/idutils.h:46-60`). The commit message says Btrfs subvolumes and NVMe partitions get dynamic device numbers, "forcing Baloo to repeatedly reindex all files".
- Consequences:
  - IDs are not stable across filesystem-identity changes. Bug 525145 (CONFIRMED, 2026-09-03), "Whole index invalidated on kernel 7.2: document IDs derive from statvfs f_fsid, which btrfs changed", reports every file getting a new ID after booting kernel 7.2. 519 775 stale documents stayed in the DB until `purge`, and the index grew from 2.06 to 2.18 GiB in 30 min. https://bugs.kde.org/show_bug.cgi?id=525145
    - The bug cites kernel commit `c2a74ed0494c` ("btrfs: derive f_fsid from on-disk fsid and dev_t"). Its last comment says the behaviour is reverted in 7.3. **Unverified**: I did not read the kernel commit or the 7.3 fix. This machine runs 7.3.0.
  - Older Btrfs incarnations: bug 402154 (RESOLVED FIXED), "Baloo reindexes everything after every reboot when using BTRFS filesystem", https://bugs.kde.org/show_bug.cgi?id=402154; bug 401863 (FIXED), "baloo creates multiple entries for files residing on multi-device btrfs file systems", https://bugs.kde.org/show_bug.cgi?id=401863.
  - Network mounts: bug 471289, "automount … nfs resource changes inode on every mount and so reindexes is triggered", https://bugs.kde.org/show_bug.cgi?id=471289
  - Inode reuse: when a file is deleted and recreated while Baloo is down, the same docId can point at a new file. The extractor detects id/path mismatches (`src/file/extractor/app.cpp:265-271`, `:162-170`).
  - Nothing garbage-collects unreachable old IDs (see §1.6). This is why stale entries accumulate.

---

## 4. Filename search vs content search

### 4.1 Term generation (`TermGenerator`)
- Normalization: NFKD, then lowercase, then strip combining marks, then NFKC (`src/engine/termgenerator.cpp:17-32`). This makes matching accent-insensitive.
- Splitting uses `QTextBoundaryFinder::Word` (ICU word boundaries). `_` is replaced by a space first (`:66`). Punctuation, marks and spaces are skipped (`:70-72`). CJK runs get special handling (`:85-97`).
- Terms are truncated to **25 UTF-16 code units** (`src/engine/termgenerator.h:39`, `src/engine/termgenerator.cpp:39-43`).
- Prefixes partition the term space in `postingdb`:
  - `F` filename (`src/engine/termgenerator.cpp:125-141`)
  - `M` mimetype (`src/file/basicindexingjob.cpp:229-233`)
  - `T<n>` KFileMetaData type (`:239-254`)
  - `TA` and `TAG-` for tags, `R` rating, `C` comment (`:46-60`)
  - `X<prop>-` extracted properties (`src/lib/searchstore.cpp:86-87`)
  - no prefix for plain content
- A multi-word name gets positional terms so phrase queries work. A single-word name gets a bare term (`src/engine/termgenerator.cpp:128-140`).

### 4.2 Query execution
- `Baloo::Query::exec()` parses the string (`QueryParser`, supports `AND`/`OR`, `prop:value`, comparators), adds `type:` and `includefolder` terms, then calls `SearchStore::exec` (`src/lib/query.cpp:161-200`).
- A bare word expands to `content OR filename` (`src/lib/searchstore.cpp:351-355`).
- Contains semantics: a single term of **≥3 bytes (prefix included)** becomes a **prefix (StartsWith) query**. Shorter terms need an exact match (`src/lib/searchstore.cpp:120-129`). Multi-word values become phrase queries over `positiondb` (`:91-118`; `src/engine/transaction.cpp:350-371`).
- The prefix query is an LMDB cursor `MDB_SET_RANGE` scan over all keys with that prefix. Each matching posting list is decoded and the lists are OR-merged (`src/engine/postingdb.cpp:158-199`).
- **There is no infix/substring or wildcard support.** "testf" finds `testfile`, "esttfile" does not. Bug 333037 (REOPENED since 2014): https://bugs.kde.org/show_bug.cgi?id=333037. The query parser has no `*` handling.
- **Sorting cost:** `SortAuto` is the default (`src/lib/query.h:147-153`). With it, `SearchStore::exec` walks **every** matching docId, looks up its mtime, sorts, and only then applies offset/limit (`src/lib/searchstore.cpp:175-210`). `setLimit(10)` does not bound the work. Each returned path then costs one `idfilename` lookup per component (§3.2).

---

## 5. Metadata and content extraction

- The scheduler runs `FileContentIndexer` on a 1-thread pool (`src/file/fileindexscheduler.cpp:43`). It spawns one long-lived child, `baloo_file_extractor` (`src/file/filecontentindexer.cpp:40`; `src/file/extractorprocess.cpp:55-57`), and talks to it over stdin/stdout with a small binary pipe protocol (`src/file/extractor/commandpipe.cpp`).
- Batch loop (`src/file/filecontentindexer.cpp:86-147`):
  1. fetch 40 pending ids,
  2. send them to the child,
  3. wait for "batch finished".
  - On a child crash or non-zero exit (exit codes 1/2/3/253, `src/file/extractorprocess.cpp:31-53`), the batch size is **halved**. When the batch size reaches 1, that id goes into `failediddb` (`src/file/filecontentindexer.cpp:121-136`; `src/file/filecontentindexerprovider.cpp:31-39`). This is a bisection crash isolator.
- Inside the extractor (`src/file/extractor/app.cpp`):
  - The real mimetype is computed with `KFileMetaData::MimeUtils::strictMimeType` (content sniffing; `:228`). Excluded mimetypes are skipped (`:229-236`).
  - `text/*` and `application/mbox` ≥ **10 MiB** are skipped (`:238-250`). No other size cap exists in Baloo.
  - Basic indexing runs again, then **all KFileMetaData extractor plugins for the mimetype run in-process** (`:257-281`). The plugins are loaded from `kf6/kfilemetadata` (`kfilemetadata:src/extractorcollection.cpp:57`).
  - **No per-file timeout.** The extractor is a single-threaded event loop with no watchdog; a hung plugin stalls indexing. KFileMetaData's *external* extractors (separate processes) use `QProcess::waitForFinished()` with Qt's default 30 s (`kfilemetadata:src/externalextractor.cpp:108`).
  - Extracted text has no upper bound. It all goes through `TermGenerator` with positions (`src/file/extractor/result.cpp:69-90`).
  - Pacing comes from **KIdleTime**. After 1 min of user idle the extractor runs "fast": 0 ms between files. When the user is active it adds 10 ms per file and 500 ms per batch/commit (`src/file/extractor/app.cpp:40-50`, `:102-104`, `:119`, `:190`).
- After each committed batch, a D-Bus signal `org.kde /files changed(as)` lists the updated files (`src/file/filecontentindexer.cpp:22-29`). `Baloo::FileMonitor` (`src/lib/filemonitor.cpp:26`) and the KDED search module consume it.

---

## 6. Scheduling, concurrency and resources

- **State machine** (`src/file/fileindexscheduler.cpp:72-192`), one job at a time, in priority order:
  1. FirstRun
  2. NewFiles
  3. ModifiedFiles
  4. XAttrFiles
  5. (Startup gate: wait)
  6. **on battery: stop** (LowPowerIdle)
  7. StaleIndexEntriesClean
  8. ContentIndexing
  9. UnindexedFileCheck
  10. Idle

  State values exposed over D-Bus are listed in `src/engine/indexerstate.h:19-32`.
- **Startup delay:** housekeeping and content indexing wait 5 s after start (`src/file/mainhub.cpp:35-39`).
- **Power:** listens to `org.freedesktop.PowerManagement.PowerSaveStatusChanged` and calls `GetPowerSaveStatus` (`src/file/powerstatemonitor.cpp:23-45`). It assumes "on battery" until the first reply (`:20`). Going on battery aborts content indexing (`src/file/fileindexscheduler.cpp:241-250`). Watch-driven basic indexing continues.
- **Idle detection:** KIdleTime in the extractor (§5) only changes pacing, not on/off.
- **CPU/IO priority:**
  - `baloo_file` sets ioprio `IDLE` (fallback BE/7), `SCHED_BATCH` and nice 19 (`src/file/main.cpp:28-30`; `src/file/priority.cpp:24-57`).
  - `baloo_file_extractor` sets ioprio IDLE, **`SCHED_IDLE`** and nice 19 (`src/file/extractor/main.cpp:21-23`; `src/file/priority.cpp:59-69`).
- **systemd unit** (installed `/usr/lib/systemd/user/kde-baloo.service`; template `src/file/kde-baloo.service.in:1-20`):
  ```ini
  Slice=background.slice
  CPUWeight=1
  IOWeight=1
  MemoryHigh=25%
  ExecCondition=/usr/bin/kde-systemd-start-condition --condition "baloofilerc:Basic Settings:Indexing-Enabled:true"
  ```
  - The installed unit also has `After=graphical-session.target` / `After=plasma-workspace.target`. That matches master, not the v6.31.0 tag; it looks like a distro backport.
  - History of the limit: `MemoryHigh=512M` was added in `db223ed1` (2023-03-28, "aggressively resource constrain baloo_file"). `50da6058` (2026-04-04, "Fix broken memory limit") replaced it with `25%`. That commit says the low limit made "the daemon and extractor become apparently live-locked" and "may be hit by indexing e.g. a single PDF file". Lesson: **cgroup `MemoryHigh` throttles; it does not bound working sets.** The design itself has to be memory-bounded.
- **Crash policy:** `KCrash::AutoRestart` (`src/file/main.cpp:49-50`).
- **Activation:** in 6.31 the daemon starts from the systemd unit or XDG autostart (`src/file/baloo_file.desktop.in`). D-Bus activation (`org.kde.baloo.service`) arrived only after 6.31 (master commit `cde9e1b`, "Make the main daemon DBus-activatable").

---

## 7. Integration surface

### 7.1 KRunner file search
- It is **not** an in-process plugin. `baloosearch` is a **D-Bus runner**. The executable is `/usr/lib/baloorunner` (`plasma-workspace:runners/baloo/CMakeLists.txt`). Installed pieces:
  - `/usr/share/krunner/dbusplugins/plasma-runner-baloosearch.desktop`:
    ```ini
    X-Plasma-API=DBus
    X-Plasma-DBusRunner-Service=org.kde.runners.baloo
    X-Plasma-DBusRunner-Path=/runner
    X-Plasma-Request-Actions-Once=true
    X-Plasma-Runner-Min-Letter-Count=2
    X-Plasma-Runner-Unique-Results=true
    X-Plasma-Runner-Weak-Results=true
    ```
  - `/usr/share/dbus-1/services/org.kde.runners.baloo.service` with `SystemdService=plasma-baloorunner.service`.
  - `/usr/lib/systemd/user/plasma-baloorunner.service` with `Type=dbus`, `TimeoutSec=5sec`, `Slice=background.slice`, `Restart=no` ("as we're dbus activated anyway").
- Behaviour of `Match(q)` (`plasma-workspace:runners/baloo/baloosearchrunner.cpp:72-166`):
  - It runs **10 sequential `Baloo::Query` executions**, one per type: Audio, Image, Video, Spreadsheet, Presentation, Folder, Document, Archive, Text, and untyped. Each has `setLimit(10)` and default `SortAuto`, so the full-result sort from §4.2 happens up to 10× per keystroke.
  - Relevance starts at .75 and drops by .05 per result.
  - `categoryRelevance` is Highest for an exact filename or basename match, High for a substring match, Low otherwise (`:143-149`).
  - Properties sent: `urls`, `subtext` (tilde-collapsed parent dir) and `category`.
  - Actions: one, `openParentDir`. `Run` uses `KIO::OpenUrlJob` with the XDG activation token.
- Bug 505838, "Using KRunner to make calculation result in huge RAM consumption by baloorunner" (RESOLVED FIXED), https://bugs.kde.org/show_bug.cgi?id=505838

### 7.2 KIO workers and Dolphin
- `baloosearch:/` (`src/kioworkers/search/baloosearch.json`, maxInstances 20)
- `tags:/` (`src/kioworkers/tags/tags.json`, writable: makedir, moving, deleting)
- `timeline:/` (`src/kioworkers/timeline/timeline.json`)
- A KDED module `baloosearchmodule` refreshes open search folders from `/files changed` (`src/kioworkers/kded/`).

### 7.3 Public KF6 API (`KF6::Baloo`)
- `Baloo::Query`: `setSearchString`, `setTypes`/`addType`, `setLimit`, `setOffset`, `setDateFilter`, `setSortingOption(SortNone|SortAuto)`, `setIncludeFolder`, `exec() -> ResultIterator`, `toSearchUrl`/`fromSearchUrl`, JSON round-trip (`src/lib/query.h:57-209`).
- `Baloo::File(path).load()`, then `properties()` / `property()` (`src/lib/file.h:33-71`).
- `Baloo::IndexerConfig`: `fileIndexingEnabled`, include/exclude folders/filters/mimetypes, `indexHidden`, `onlyBasicIndexing`, `refresh()` (`src/lib/indexerconfig.h:36-144`).
- `Baloo::FileMonitor` emits `fileMetaDataChanged` (`src/lib/filemonitor.h:59-63`).
- All of these open the LMDB file directly, read-only, in the client process (`src/lib/searchstore.cpp:149-156`). There is **no query daemon**. Every client maps the 256 GiB region (virtual) and pays page-cache misses itself.

### 7.4 Daemon D-Bus interfaces (service `org.kde.baloo`)
These are generated from the C++ headers (`src/dbus/CMakeLists.txt`) and installed to `/usr/share/dbus-1/interfaces/`.

| Object path | Interface | Members |
|---|---|---|
| `/` | `org.kde.baloo.main` | `quit()`, `updateConfig()` |
| `/scheduler` | `org.kde.baloo.scheduler` | prop `state` (i); signal `stateChanged(i)`; `suspend()`, `resume()`, `getRemainingTime() -> u`, `checkUnindexedFiles()`, `checkStaleIndexEntries()`, `getBatchSize() -> u` |
| `/fileindexer` | `org.kde.baloo.fileindexer` | prop `currentFile` (s); signals `startedIndexingFile(s)`, `finishedIndexingFile(s)`, `finishedIndexingFile1(s,i,i)`, `committedBatch(u,u)`; `registerMonitor()`, `unregisterMonitor()` |
| `/files` | `org.kde` | signal `changed(as)` |

- Registration sites: `src/file/mainhub.cpp:31-33`, `src/file/fileindexscheduler.cpp:56-57`, `src/file/filecontentindexer.cpp:55-56`.
- `org.kde.baloo.file.indexer.xml` is an empty stub "to avoid compilation error in plasma-desktop. DO NOT USE."
- `org.kde.BalooWatcherApplication.renamedFiles(s,s,as)` is a client-side callback interface.

---

## 8. Recovery, corruption and reindexing

- **Schema version:** `s_dbVersion = 2` (`src/file/migrator.cpp:28`). A mismatch with `baloofilerc [General] dbVersion` deletes `index` and `index-lock`, which forces a first run (`:35-50`).
  - Bug 477068 (UNCONFIRMED), "DB must be purged whenever the DB scheme/contents change in an incompatible way": https://bugs.kde.org/show_bug.cgi?id=477068
  - v6.31 still carries "compatibility hack … remove on next DB bump" code (`src/lib/searchstore.cpp:102-106`).
- **Corruption at open:** if `open(CreateDatabase)` fails for any reason other than an invalid path, `baloo_file` **deletes the index and starts over** (`src/file/main.cpp:67-82`). Extractor commit failure exits with status 2. When the daemon cannot record a failed id, it `exit(1)`s with "DB likely is in a bad state" (`src/file/filecontentindexer.cpp:121-128`).
  - Crash bugs rooted in corrupt DB content: 434926, "Crash in Baloo::IdFilenameDB::get() after Baloo::DocumentUrlDB::get" (CONFIRMED), https://bugs.kde.org/show_bug.cgi?id=434926; 367480, "baloo_file crashes in Baloo::PostingCodec::decode" (FIXED), https://bugs.kde.org/show_bug.cgi?id=367480
- **`balooctl`** (`src/tools/balooctl/main.cpp`):
  - `purge` quits the daemon, deletes `index` and restarts it (`:149-181`).
  - `check` calls `checkUnindexedFiles()` (`:195-199`).
  - `failed` lists up to 128 entries from `failediddb` (`:211-235`).
  - `clear` and `index` operate on individual files.
  - The DB consistency checker `checkDb` is **commented out** (`:288-302`). There is no fsck.
- **Failed IDs:** ids in `failediddb` are removed from the pending set (`src/file/filecontentindexerprovider.cpp:31-39`). They leave `failediddb` on successful re-extraction (`src/file/extractor/app.cpp:173`) or on document removal (`src/engine/writetransaction.cpp:119,136`). **Unverified**: the exact trigger for a retry. Most likely a later mtime change re-marks the file through `UnindexedFileIndexer` (`src/file/unindexedfileindexer.cpp:41-43`).
- **Deleted-but-still-found entries:** bug 353874 (ASSIGNED since 2015), "Baloo does not remove deleted files from index", https://bugs.kde.org/show_bug.cgi?id=353874; bug 525652 (CONFIRMED), "baloo clear directory leaks files in the index even after deletion", https://bugs.kde.org/show_bug.cgi?id=525652

---

## 9. Measurable bottlenecks (code path → evidence)

| # | Bottleneck | Code path | Upstream evidence |
|---|---|---|---|
| B1 | Write amplification: whole posting/position list rewritten per touched term per commit; raw 8-byte postings | `src/engine/writetransaction.cpp:293-325`; `src/codecs/postingcodec.cpp:12-27` | 442453 (CONFIRMED): 20-100 MB/s writes on delete; 2026 repro "466 GB writes, 8.39 GB memory peak" |
| B2 | Index growth / stale docs after ID change; no GC of unreachable IDs; LMDB never shrinks | `src/engine/idutils.h:22-60`; `src/file/indexcleaner.cpp:46-55`; no `mdb_env_copy` | 525145 (kernel 7.2 f_fsid, +stale 519 775 docs), 402154, 401863, 471289; 449713 "huge index after deleting a folder" (dup) |
| B3 | Lost events: overflow signal unconnected; watch-limit disables *all* watching | `src/file/kinotify.cpp:326-335` vs `src/file/filewatch.cpp:47-54`; `src/file/filewatch.cpp:153-166` | 437754 (CONFIRMED), 381622 |
| B4 | Per-login O(N) work: full watch-install walk plus full stat-and-lookup reconciliation walk | `src/file/kinotify.cpp:150-179`; `src/file/unindexedfileiterator.cpp:70-124` | 456108 "reindexes files with date of 0 or prior to UNIX epoch on every restart" (CONFIRMED) |
| B5 | Unbounded extractor memory; in-process plugins, no timeout, unbounded text | `src/file/extractor/app.cpp:238-281` | 443547 "Baloo_file_extractor uses all available memory and never finishes running" (FIXED); 447681 mbox (dup); 489588 "huge memory usage when indexing unzipped files" (NEEDSINFO); `50da6058` live-lock under `MemoryHigh=512M` |
| B6 | Query cost scales with match count, not limit (SortAuto), ×10 in the KRunner runner | `src/lib/searchstore.cpp:175-210`; `plasma-workspace:runners/baloo/baloosearchrunner.cpp:91-103` | 492583 "Enabled file indexing causes huge slow down when opening Dolphin" (dup; title only) |
| B7 | Prefix-only filename matching (no substring) | `src/lib/searchstore.cpp:120-129`; `src/engine/postingdb.cpp:158-199` | 333037 (REOPENED) |
| B8 | First-run memory unbounded within each 20k-doc transaction | `src/file/firstrunindexer.cpp:64-75` (FIXME) | – |
| B9 | Long-lived readers (e.g. `balooctl status`) during deletes → memory/index growth | – | 437754. **Unverified**: mechanism (LMDB MVCC pins old pages while a reader is open) |
| B10 | Throttling makes indexing slow and noisy rather than bounded | `CPUWeight=1`, SCHED_IDLE, KIdleTime pacing | 476752 "Option to un-throttle Baloo File Indexing Process" |

All bug URLs follow the form `https://bugs.kde.org/show_bug.cgi?id=<n>`. Every number above was fetched via the REST API on 2026-10-31.

---

## 10. KRunner D-Bus protocol (KF 6.31), the exact contract FerroSeek must implement

Sources: `/usr/share/dbus-1/interfaces/kf6_org.kde.krunner1.xml`, `krunner:src/dbusrunner.cpp`, `krunner:src/dbusutils_p.h`, `krunner:src/runnermanager.cpp`, `krunner:src/kpluginmetadata_utils_p.h`, https://develop.kde.org/docs/plasma/krunner/metadata/

### 10.1 Interface `org.kde.krunner1` (the runner exports it at its object path)
| Method | Signature | Semantics |
|---|---|---|
| `Match` | `(s query) -> a(sssida{sv})` | Return matches. The XML says: "Multiple Match calls may be made with new queries before a call has returned" (`kf6_org.kde.krunner1.xml:76-77`). |
| `Actions` | `() -> a(sss)` | Each struct is `(id, text, iconName)` (wire order per `krunner:src/dbusutils_p.h:68-90`). Called once per match session, or once per service lifetime if `X-Plasma-Request-Actions-Once=true` (`krunner:src/dbusrunner.cpp:115-141`). |
| `Run` | `(s matchId, s actionId) -> ()` | `actionId` is empty for the default action. Sent with `QDBus::NoBlock` (`krunner:src/dbusrunner.cpp:285-289`). |
| `SetActivationToken` | `(s token) -> ()` | On Wayland, called just before `Run` with an XDG activation token (`krunner:src/dbusrunner.cpp:291-300`). Use it as `XDG_ACTIVATION_TOKEN` / startup id when launching. |
| `Config` | `() -> a{sv}` | **Only when `X-Plasma-API=DBus2`** (`krunner:src/dbusrunner.cpp:33,106-113`). Called at runner init (`krunner:src/abstractrunner.cpp:103-106`); matching is suspended until it replies (`krunner:src/dbusrunner.cpp:110,143-174`). Keys: `MatchRegex` (s), `MinLetterCount` (i), `TriggerWords` (as), `Actions` (`a(sss)`). Uses the first matching service only (`:145`). |
| `Teardown` | `() -> ()` | Async, at the end of a match session, **only if `Match` was called** in that session (`krunner:src/dbusrunner.cpp:84-93`). |

### 10.2 Match struct `(sssida{sv})` (`krunner:src/dbusutils_p.h:18-26`)
1. `s id`: opaque match id, passed back to `Run`.
   - With `X-Plasma-Runner-Unique-Results=true` it is kept verbatim and used to de-duplicate across runners.
   - Otherwise KRunner stores it as `<runnerId>_<id>` and strips that prefix before `Run` (`krunner:src/querymatch.cpp:52-62`; `krunner:src/dbusrunner.cpp:274-278`).
   - Baloo uses the `file://` URL as id. That lets its weak results be replaced by another runner returning the same id (`krunner:src/runnercontext.cpp:50-60`).
2. `s text`: display text.
3. `s iconName`: themed icon name.
4. `i categoryRelevance`: `KRunner::QueryMatch::CategoryRelevance`, values `Lowest=0, Low=30, Moderate=50, High=70, Highest=100` (`krunner:src/querymatch.h:71-77`). The default is Lowest (`dbusutils_p.h:23`). It "only affects the sorting of categories and not the sorting within the category" (`querymatch.h:79-83`); the highest value in a category wins.
   - The XML comment still says "Type (see KRunner::QueryMatch::Type)". That is the KF5 name, whose values were NoMatch 0 / Completion 10 / Possible 30 / Informational 50 / Helper 70 / Exact 100 (as mirrored by the `krunner` crate docs, https://docs.rs/krunner/0.1.1/krunner/enum.MatchType.html).
5. `d relevance`: ordering inside a category, "a number between 0 and 1" (`krunner:src/querymatch.h:123-128`).
6. `a{sv} properties`: the only keys read (`krunner:src/dbusrunner.cpp:176-229`) are:
   - `urls` (`as`, parsed with `QUrl::fromStringList`, so send `file:///…` URLs): drives drag-and-drop and copy-URL behaviour.
   - `category` (`s`): category title for grouping.
   - `subtext` (`s`): second line.
   - `multiline` (`b`).
   - `actions` (`as` of action ids). If absent, **all** actions are shown. Send an empty list to show none.
   - `icon-data` (`(iiibiiay)` = width, height, rowstride, hasAlpha, bitsPerSample, channels, data). Only 8-bit, 3 or 4 channels, width and height below 2048 (`:306-343`). Overrides `iconName`.
   - Any other key is ignored.

### 10.3 `.desktop` metadata
- Parsed by `krunner:src/kpluginmetadata_utils_p.h:27-59`.
- Search path: every `$XDG_DATA_DIRS/krunner/dbusplugins` plus `~/.local/share/krunner/dbusplugins` (`QStandardPaths::locateAll(GenericDataLocation, "krunner/dbusplugins")`, `krunner:src/runnermanager.cpp:508-517`).
- Precedence: unique by filename. A C++ plugin with the same id wins (`knownRunnerIds`).
- Keys read:
  - `X-KDE-PluginInfo-Name` is the **plugin id** and is required.
  - `Name`, `Icon`, `Comment`.
  - `X-KDE-PluginInfo-EnabledByDefault` (bool).
  - `X-Plasma-API`: `DBus` or `DBus2`. Any value starting with `DBus` selects `DBusRunner` (`krunner:src/runnermanager.cpp:182-196`). Only exactly `DBus2` enables `Config`.
  - `X-Plasma-DBusRunner-Service`: required. A trailing `*` means "all currently registered names with this prefix", tracked via `NameOwnerChanged` (`krunner:src/dbusrunner.cpp:42-82`).
  - `X-Plasma-DBusRunner-Path`: defaults to `/runner` (`:30`).
  - `X-Plasma-Runner-Min-Letter-Count` (int) and `X-Plasma-Runner-Match-Regex`: evaluated **client-side before any D-Bus call** (`krunner:src/runnermanager.cpp:612-621`).
  - `X-Plasma-Request-Actions-Once` (bool).
  - `X-Plasma-Runner-Unique-Results` and `X-Plasma-Runner-Weak-Results` (bool).
  - `X-Plasma-Runner-Syntaxes` / `X-Plasma-Runner-Syntax-Descriptions`.
  - `X-KDE-ConfigModule`.
- Keys that do not exist or are ignored:
  - **`X-Plasma-DBusRunner-Activation` does not exist** in KRunner 6.31. It appears nowhere in `krunner/src`.
  - `X-Plasma-Runner-Lifecycle-Methods` (present in the installed `plasma-runnners-activities.desktop`) is **ignored** by the parser.
- **D-Bus activation:** a non-wildcard service is called by name without checking that it exists, explicitly "as it could be used with DBus-activation" (`krunner:src/dbusrunner.cpp:79-82`).
  - The bus daemon starts the service if a `.service` file exists in `~/.local/share/dbus-1/services/` (or `/usr/share/dbus-1/services/`).
  - The official template's `install.sh` writes `$XDG_DATA_HOME/dbus-1/services/org.kde.<name>.service` with `[D-BUS Service] Name=… Exec=…`, so the script "gets executed when KRunner requests matches and it does not need to be autostarted" (`krunner:templates/runner6python/README.md`, `install.sh`).
  - **Wildcard names are never activated**; they must already be running.
  - Baloo's runner adds `SystemdService=` so the activated process runs inside a systemd unit (§7.1).
- **Enable/disable:** `krunnerrc`, group `[Plugins]`, key `<pluginId>Enabled=true|false` (`krunner:src/runnermanager.cpp:401-402`; `KPluginMetaData::isEnabled` reads `pluginId() + "Enabled"`, `/usr/include/KF6/KCoreAddons/kpluginmetadata.h:510`). The default is `EnabledByDefault`. KRunner reloads on change via `KConfigWatcher` (`krunner:src/runnermanager.cpp:296-315`). Per-runner config lives under `krunnerrc [Runners][<id>]` (`krunner:src/abstractrunner.cpp:49-53`).
- After installing a new `.desktop`, the template uses `kquitapp6 krunner`.

### 10.4 Call pattern and timing
- Every keystroke that passes the letter-count and regex filters produces an **async** `Match` call with no cancellation (`krunner:src/dbusrunner.cpp:230-268`).
- The timeout is QtDBus's default, "generally, 25 seconds" (https://doc.qt.io/qt-6/qdbusconnection.html, `asyncCall`).
- Replies arriving after the query changed are **dropped** (`RunnerContext::addMatches` returns early when the context is invalid, `krunner:src/runnercontext.cpp:182-197`). Late answers only waste work.
- UI updates are throttled to every 250 ms (`krunner:src/runnermanager.cpp:72`).
- The D-Bus service must therefore handle overlapping `Match` calls, ideally with **latest-query-wins** cancellation inside the service, and answer well under ~50 ms to feel instant. The 50 ms figure is design guidance, not a KRunner constant.
- Official docs for C++ runners: "For each letter typed, the plugin's match() method gets called in a new thread" (https://develop.kde.org/docs/plasma/krunner/).

### 10.5 Minimal FerroSeek runner files
```ini
# ~/.local/share/krunner/dbusplugins/ferroseek.desktop
[Desktop Entry]
Type=Service
Name=FerroSeek
Icon=system-search
X-KDE-PluginInfo-Name=ferroseek
X-KDE-PluginInfo-EnabledByDefault=true
X-Plasma-API=DBus2
X-Plasma-DBusRunner-Service=dev.ferroseek.Runner
X-Plasma-DBusRunner-Path=/runner
X-Plasma-Request-Actions-Once=true
X-Plasma-Runner-Min-Letter-Count=2
X-Plasma-Runner-Unique-Results=true
X-Plasma-Runner-Weak-Results=true
```
```ini
# ~/.local/share/dbus-1/services/dev.ferroseek.Runner.service
[D-BUS Service]
Name=dev.ferroseek.Runner
Exec=/usr/bin/ferroseekd
SystemdService=ferroseek.service   # optional
```
- `Unique-Results` + `Weak-Results` with `file://` ids mirrors Baloo, so the two runners de-duplicate if both are enabled.
- Service and executable names are placeholders.

---

## 11. Existing alternatives

| Project | What it is | Update model | Notes |
|---|---|---|---|
| **plocate** (C++, GPL-2+) | locate replacement with an inverted index over **trigrams** of paths ("creating an inverted index over trigrams", https://plocate.sesse.net/) | Batch: `updatedb` from a systemd timer (`README`). No live updates. | Header holds a trigram hash table and zstd dictionary offsets (`plocate-src/db.h:6-45`). Posting lists are **delta-encoded and TurboPFor-style bit-packed in 128-int blocks** (`database-builder.cpp:56-100`). Filenames are concatenated into blocks and **zstd-compressed with a shared dictionary**; a docid is a filename block (`database-builder.cpp:30,241-249`). Uses io_uring when available (`README`; binary links `liburing`, `libzstd`). Benchmark on its homepage: 27 M files, 0.008 s vs mlocate 20.1 s; DB 466 MB vs 1.1 GB. Patterns are substrings by default; it only scans the whole DB for patterns under 3 bytes or regexes (`man plocate`). |
| **mlocate** | Predecessor; per-directory DB scanned linearly at query time | Batch `updatedb` | Superseded by plocate (comparison above). **Unverified**: internal format details, not researched. |
| **FSearch** (C/GTK3, GPL-2.0, ~4.5k★) | "Everything"-style instant filename search; wildcards, regex, fast sort (https://github.com/cboxdoerfer/fsearch) | Rescans its own in-memory DB | **Unverified**: whether current releases do inotify/fanotify live updates. No KRunner integration found in its README. |
| **GlintIndex** (Rust, Apache-2.0, created 2026-07-07, 0★) | "fast, local desktop search engine built in Rust": Tantivy full-text index, 27+ file types, CLI and egui GUI (https://github.com/riccione/glintindex) | `index` command is incremental (skip unchanged, reindex modified, remove missing). Depends on `notify` 8.2. **Unverified**: whether it runs as a persistent watcher daemon. | Content-first, source-code oriented. No KRunner or D-Bus integration in its README. |
| **`krunner` crate** 0.1.1 (Rust) | "A crate for writing KRunner plugins in Rust" (https://crates.io/crates/krunner); traits `Runner`, `Action`; `tokio` and `derive` features | – | Last published 2023-09-28. Depends on `dbus` 0.9 (libdbus C binding) and `dbus-crossroads`. Its `MatchType` enum uses the KF5 names (§10.2). The repository URL `github.com/pluiedev/krunner` returned 404 on 2026-10-31. **Treat as unmaintained**; implementing `org.kde.krunner1` directly with `zbus` (pure Rust) is about six methods. |

How FerroSeek can differ, based on the evidence above:
1. **Real-time and robust:** unlike plocate/mlocate (batch), and unlike Baloo, which drops overflow events and disables watching at the limit.
2. **Native KRunner D-Bus runner inside the daemon:** queries hit warm in-memory structures with latest-query-wins. Baloo runs a separate `baloorunner` doing 10 cold LMDB queries per keystroke.
3. **Low idle cost:** no full walk on every login when a persisted journal or `fanotify` makes that avoidable (§12). Bounded memory by design rather than cgroup throttling.
4. **Substring/fuzzy filename matching** (trigrams), which Baloo has never had (333037).

---

## 12. Implications for FerroSeek

Each decision cites the evidence that motivates it.

1. **Do not key documents on `(st_dev, st_ino)` or `f_fsid` alone.** Use an internal monotonically assigned `u64` doc id. Keep a secondary map `(fs_key, ino, generation?) -> doc_id` that can be rebuilt. Treat `fs_key` (prefer filesystem UUID from `/proc/self/mountinfo` or libblkid over `f_fsid`/`st_dev`) as a re-validatable hint. On mismatch, re-key by path instead of reindexing. (§3.4; bugs 402154, 525145, 401863, 471289)
2. **Garbage-collect unreachable documents.** Run a reconciliation pass that deletes ids not seen during a walk (mark-and-sweep generation counter). Baloo never removes stale ids. (§1.6, B2)
3. **Store paths as parent pointers** (`id -> (parent, name)`), as Baloo does. That makes directory renames O(1). Cache hot path prefixes in memory so query results do not pay one lookup per component. (§2.3, §3.2)
4. **Handle `IN_Q_OVERFLOW` by scheduling a targeted rescan** of the watched roots, using directory mtime/ctime to prune, instead of only logging. (§2.2, bug 437754)
5. **Never go dark on `ENOSPC`.** Keep existing watches, record the unwatched subtrees, and fall back to periodic mtime polling for those. Surface the condition via D-Bus and logs, and suggest raising `fs.inotify.max_user_watches`. (§2.4)
6. **Consider fanotify where privileged.** `FAN_MARK_FILESYSTEM` needs `CAP_SYS_ADMIN`; unprivileged fanotify (≥5.13) may only mark inodes (fanotify_init(2) and fanotify_mark(2) man pages). The default design should be inotify plus a small optional privileged helper, or plain inotify with good overflow and limit handling.
7. **Avoid Baloo's per-login O(N) reconciliation where possible.** Persist the last-seen directory mtimes and only descend into changed directories (as plocate's `updatedb` does with directory times, `database-builder.cpp:252-262`). Run the walk at `SCHED_IDLE` / ioprio idle. Watch installation must still walk all directories, so make it incremental and interruptible. (B4)
8. **Filename index first, content later and optional.** Baloo's filename search is word-prefix only. Index filename **trigrams** (or n-grams) with delta plus bit-packed postings (plocate design) to get substring matching, and keep word-prefix ranking on top. (§4.2, §11, bug 333037)
9. **Avoid whole-list read-modify-write.** Use an LSM or segment design: immutable compressed segments plus an in-memory delta and tombstones, merged in the background with bounded I/O. Alternatively chunk posting lists into fixed-size blocks keyed `(term, block_no)`. Baloo's single-value posting lists are the root cause of B1 (442453).
10. **Make queries bounded by `limit`.** Never sort the full match set for a KRunner query. Use top-k with a heap over a precomputed static rank (recency, depth, basename match). (§4.2 SortAuto, B6)
11. **Bound memory by construction** (fixed-size batches by bytes, streaming extraction with a hard text cap per file, per-file wall-clock timeout, extractor in a killable subprocess with `RLIMIT_AS`). Use systemd `MemoryHigh` only as a backstop. Baloo shows a cgroup limit alone causes live-locks (`50da6058`). (§5, §6, B5)
12. **Adopt Baloo's good resource defaults:** `background.slice`, `CPUWeight`/`IOWeight` low, `SCHED_IDLE` plus ioprio IDLE for bulk work, pause content work on battery via `org.freedesktop.PowerManagement` (or UPower), 5 s startup delay. Keep the live filename-update path at normal priority so search stays fresh. (§6)
13. **KRunner runner, in-process with the daemon, over `zbus`:** implement `org.kde.krunner1` exactly as in §10.
    - Use `X-Plasma-API=DBus2` only if `Config` is needed. With `DBus`, nothing is called until the first `Match`, which is better for idle cost.
    - Use a non-wildcard service name so D-Bus activation works.
    - Return `file://` ids with `Unique-Results=true` and `Weak-Results=true`.
    - Fill `urls`, `subtext` (parent dir, `~`-collapsed), `category`. Set `actions` explicitly.
    - Implement `SetActivationToken`.
    - Cancel stale `Match` work (latest-query-wins) because KRunner keeps sending without cancelling. (§10.4)
14. **D-Bus-activatable daemon with an idle-exit option.** Ship a `.service` file with `SystemdService=` and a user unit with `Type=dbus`, mirroring `plasma-baloorunner.service`. Baloo itself only became D-Bus-activatable after 6.31. (§6, §7.1)
15. **Provide recovery tooling Baloo lacks:** an online consistency check (Baloo's `checkDb` is commented out), schema-version migration without forced full rebuilds where possible, and compaction (LMDB never shrinks). (§3.1, §8)
16. **Respect existing user expectations** where cheap: import Baloo's default exclude filters and folder excludes as FerroSeek defaults (`src/file/fileexcludefilters.cpp:12-139`), and do not follow symlinks by default (`src/file/filtereddiriterator.cpp:15`).
