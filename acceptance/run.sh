#!/usr/bin/env bash
# Acceptance runs against real client sites. Client data never enters the repo: the project
# and snapshot folders are git-ignored and the table shows counts only.
#
#   acceptance/run.sh landmark                 full run: porter run --apply --json, then the table
#   acceptance/run.sh landmark --local-only    analyse, extract, normalise, schema check with the
#                                              wp-db connector only (no crawl, nothing loaded)
#   acceptance/run.sh landmark --stage normalise --stage schema   just those stages
#   acceptance/run.sh landmark --table         just print the table
#   --no-fail                                  exit 0 even when a stage fails or a count differs
#
# Exits 1 when any stage exits non-zero or any count row differs from the importer's.
#
# LANDMARK_DIR (default ~/Desktop/work/clients/landmark) is only read.
set -euo pipefail

site="${1:-}"
[[ "$site" == landmark ]] || { echo "usage: $0 landmark [--local-only] [--stage NAME]... [--table] [--no-fail]" >&2; exit 1; }
shift
local_only=0 table_only=0 no_fail=0 failed=0 stages=()
while (($#)); do
  case "$1" in
    --local-only) local_only=1 ;;
    --table) table_only=1 ;;
    --no-fail) no_fail=1 ;;
    --stage) stages+=("$2"); shift ;;
    *) echo "unknown option $1" >&2; exit 1 ;;
  esac
  shift
done

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(dirname "$here")"
LANDMARK_DIR="${LANDMARK_DIR:-$HOME/Desktop/work/clients/landmark}"
dir="$here/landmark"
project="$dir/project"
snapshot="$dir/snapshot"
[[ -d "$LANDMARK_DIR/wp-source" ]] || { echo "LANDMARK_DIR=$LANDMARK_DIR has no wp-source/" >&2; exit 1; }

cargo build --release -q -p porter-cli --manifest-path "$repo/Cargo.toml"
porter="$repo/target/release/porter"

# porter.toml from the example; the crawl connector only with a snapshot and not --local-only.
mkdir -p "$project"
[[ -f "$project/porter.toml" ]] || "$porter" --project "$project" init landmark --adapter wordpress >&2
crawl=1
if ((local_only)) || [[ ! -d "$snapshot/crawl" ]]; then
  crawl=0
  ((local_only)) || echo "no snapshot at $snapshot: running without the crawl connector" >&2
fi
CRAWL="$crawl" LANDMARK_DIR="$LANDMARK_DIR" SNAPSHOT_DIR="$snapshot" python3 -I - "$dir/porter.toml.example" >"$project/porter.toml" <<'PY'
import os, re, sys
s = open(sys.argv[1]).read()
s = s.replace('@LANDMARK_DIR@', os.environ['LANDMARK_DIR']).replace('@SNAPSHOT_DIR@', os.environ['SNAPSHOT_DIR'])
if os.environ['CRAWL'] != '1':
    s = re.sub(r'# @CRAWL@[^\n]*\n\[\[connectors\]\]\nkind = "crawl"\n[^\n]*\n', '', s)
sys.stdout.write(s)
PY

run() {
  echo "== porter $*" >&2
  "$porter" --project "$project" "$@" || { echo "(exit $?)" >&2; failed=1; }
}
if ((!table_only)); then
  if ((${#stages[@]})); then
    for s in "${stages[@]}"; do
      case "$s" in schema) run schema check ;; *) run "$s" ;; esac
    done
  elif ((local_only)); then
    # wp-db's analyse reads what extract exported, so it runs after it.
    for s in extract analyse normalise; do run "$s"; done
    run schema check
  else
    [[ -n "${PORTER_PAYLOAD_API_KEY:-}" ]] || { echo "set PORTER_PAYLOAD_API_KEY (see acceptance/landmark/README.md)" >&2; exit 1; }
    "$porter" --project "$project" --json run --apply >"$project/run.ndjson" || { echo "(porter run exit $?)" >&2; failed=1; }
  fi
fi

# The table: porter's counts against what Landmark's importer produced from the same data.
LANDMARK_DIR="$LANDMARK_DIR" python3 -I - "$project/project.db" <<'PY' || failed=1
import json, os, re, sqlite3, sys
from urllib.parse import urlparse

L = os.environ['LANDMARK_DIR']
data = lambda n: json.load(open(f'{L}/migration/data/{n}'))
db = sqlite3.connect(f'file:{sys.argv[1]}?mode=ro', uri=True)
q = lambda sql, *a: db.execute(sql, a).fetchall()

# Landmark's importer (migration/import-content.ts TYPES; a post at a page's URL is skipped)
TYPES = ['page', 'post', 'rooms', 'events', 'promotions', 'bars', 'guides', 'blog', 'neighbour', 'slides', 'popups', 'notifications']
page_links = {i['link'] for i in data('page.json')}
want = {t: len([i for i in data(f'{t}.json') if t != 'post' or i['link'] not in page_links]) for t in TYPES}

# porter: documents a load writes (published and private; drafts are skipped in the mapping)
docs = dict(q("""select json_extract(data,'$.type'), count(*) from items
  where kind='document' and connector='wordpress' and deleted_at is null
  and json_extract(data,'$.status') in ('published','private') group by 1"""))
loaded = dict(q("""select json_extract(i.data,'$.type'), count(*) from statuses s join items i on i.id=s.item_id
  where s.stage='load' and s.status='"done"' and i.kind='document' group by 1"""))

# media: one per attachment whose file exists (import-media.ts)
att = data('attachments.json')
has = lambda f: any(os.path.isfile(f'{L}/wp-source/{d}/{f}') for d in ('uploads-case-dupes', 'uploads'))
media_want = [a for a in att if has(a['file'])]
def folders(dirs):
    out = set()
    for d in dirs:
        while d and d != '.':
            out.add(d); d = os.path.dirname(d)
    return len(out)
media_n = q("select count(*) from items where kind='media' and deleted_at is null")[0][0]
attach_ok = q("""select count(*) from items where kind='document' and connector='wp-db'
  and json_extract(data,'$.label')='attachment' and deleted_at is null""")[0][0] - q("""select count(*)
  from statuses where stage='normalise' and status like '%"no file"%' and deleted_at is null""")[0][0]
porter_dirs = [r[0] for r in q("select json_extract(data,'$.folder') from items where kind='media' and deleted_at is null") if r[0]]

# redirects: import-redirects.ts replayed on its data (Redirection rows, old_site_url, hand-written rules)
# the site's own host, from its own links (as the importer's OWN pattern names it)
host = urlparse(data('page.json')[0]['link']).hostname.removeprefix('www.')
OWN = re.compile(r'^(?:https?:)?//(?:www\.)?' + re.escape(host), re.I)
def from_path(u):
    u = u.strip()
    if re.search(r'[?#,\s]', u): return None
    if re.match(r'^https?://', u, re.I) or u.startswith('//'):
        if not OWN.match(u): return None
        u = OWN.sub('', u)
    u = u if u.startswith('/') else '/' + u
    return u if u.endswith('/') or re.search(r'\.[a-z0-9]+$', u, re.I) else u + '/'
m = {}
def add(f, t):
    if f and t and f != t: m[f] = t
for r in data('redirection.json'):
    f = from_path(r['url']) if r['status'] == 'enabled' else None
    if f and f not in m: add(f, r['action_data'])
claims = [(i, str((i['acf'] if isinstance(i['acf'], dict) else {}).get('old_site_url') or '').strip())
          for t in ('page', 'rooms', 'bars', 'events') for i in data(f'{t}.json')]
claims = sorted((c for c in claims if c[1]), key=lambda c: c[0]['date'], reverse=True)
claims.sort(key=lambda c: c[0]['status'] != 'publish')
seen = set()
for i, old in claims:
    f = from_path(old)
    if f and f not in seen: seen.add(f); add(f, urlparse(i['link']).path)
# the hand-written rules, as porter.toml.example transcribes them from import-redirects.ts
for f, t in re.findall(r'\[\[redirects\]\]\nfrom = "([^"]+)"\nto = "([^"]+)"', open(os.path.dirname(sys.argv[1]) + '/porter.toml').read()):
    add(f, t)
final = 0
for f, t in m.items():
    seen = {f}
    while (from_path(t) or '') in m and from_path(t) not in seen:
        seen.add(from_path(t)); t = m[from_path(t)]
    final += from_path(t) != f
red_n = q("select count(*) from items where kind='redirect' and connector='wordpress' and deleted_at is null")[0][0]

# submissions: Ninja Forms subs with a form, plus Gravity Forms entries (import-submissions.ts)
nf, gf = {}, set()
for l in open(f'{L}/migration/data/nf-subs.tsv', encoding='utf8', errors='replace'):
    p = l.rstrip('\n').split('\t')
    if len(p) >= 4:
        nf.setdefault(p[0], False)
        nf[p[0]] |= p[2] == '_form_id'
for l in open(f'{L}/migration/data/gf-entries.tsv', encoding='utf8', errors='replace'):
    p = l.split('\t')
    if len(p) >= 2:  # a line without a form id is a stray fragment; it isn't imported
        gf.add(p[0])
subs_want = sum(nf.values()) + len(gf)
subs_n = q("select count(*) from items where kind='submission' and connector='wordpress' and deleted_at is null")[0][0]

rows = [(t, want[t], docs.get(t, 0), loaded.get(t, '-')) for t in TYPES]
rows += [
    ('media (attachments with a file)', len(media_want), f'{media_n} files ({attach_ok} attachments)', '-'),
    ('media folders', folders(os.path.dirname(a['file']) for a in media_want), folders(porter_dirs), '-'),
    ('redirects', final, red_n, '-'),
    ('submissions', subs_want, subs_n, '-'),
]
differs = False
print(f"{'':34} {'Landmark':>9} {'porter':>28} {'loaded':>7}")
for name, w, p, l in rows:
    mark = '' if str(w) == str(p).split(' ')[0] else '  <-'
    differs |= bool(mark)
    print(f'{name:34} {w:>9} {str(p):>28} {str(l):>7}{mark}')
print('\nopen issues, by stage and kind:')
for st, k, sev, n in q("""select stage, kind, severity, count(*) from issues where deleted_at is null
  and resolution='open' group by 1,2,3 order by 1,2"""):
    print(f'  {st:10} {k:20} {sev:8} {n}')
sys.exit(1 if differs else 0)
PY

if ((failed && !no_fail)); then
  echo "acceptance: a stage failed or a count differs (--no-fail to ignore)" >&2
  exit 1
fi
