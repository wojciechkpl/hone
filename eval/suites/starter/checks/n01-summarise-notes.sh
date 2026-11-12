# requires: python3
# Success check for n01-summarise-notes. Runs in the task's working-directory copy.
#
# summary.md must state the final decisions (normalised, so paraphrases and spelling
# variants pass), must not present superseded values as decisions, must leave out the
# noise from the notes, and must be a summary: at most half the size of the notes.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import glob
import os
import re
import sys
import unicodedata


def norm(text):
    text = unicodedata.normalize("NFKD", text)
    text = "".join(c for c in text if not unicodedata.combining(c))
    return text.lower()


try:
    raw = open("summary.md", encoding="utf-8").read()
except FileNotFoundError:
    print("summary.md is missing")
    sys.exit(1)

# The cap comes from the suite's pristine notes, not from the agent's copy (it could delete them).
suite = os.environ["EVAL_SUITE_DIR"]
notes = sum(len(open(p, encoding="utf-8").read()) for p in glob.glob(f"{suite}/n01-summarise-notes/notes/*.md"))
if not notes:
    print("the suite's notes are missing")
    sys.exit(1)
if len(raw) > notes // 2:
    print(f"summary.md is {len(raw)} bytes; a summary must be at most half of the notes ({notes // 2})")
    sys.exit(1)

text = norm(raw)
# split into statements: lines, sentences and semicolon clauses
segments = [s.strip() for s in re.split(r"[\n;]|\.(?:\s|$)", text) if s.strip()]

MONTHS = {"3": "(?:march|mar)", "4": "(?:april|apr)"}


def date(month, day):
    m = MONTHS[str(month)]
    return rf"(?:{m}\s+0?{day}(?:st|nd|rd|th)?\b|\b0?{day}(?:st|nd|rd|th)?\s+(?:of\s+)?{m}\b|2026-0{month}-{day}\b|\b0?{month}/{day}\b|\b{day}/0?{month}\b)"


APRIL_14 = date(4, 14)
MARCH_30 = date(3, 30)
MARCH_31 = date(3, 31)
B42 = r"\b42(?:[,.  ]?000\b|\s?k\b)"
B38 = r"\b38(?:[,.  ]?000\b|\s?k\b)"
DROP = r"dropp|reject|instead|rather|raised|from|was\b|were\b|initial|earlier|proposal|proposed|previous|replac|superseded|not\b|no longer|originally|old\b"

problems = []


def need(label, ok):
    if not ok:
        problems.append(f"missing: {label}")


def with_all(*patterns):
    return [s for s in segments if all(re.search(p, s) for p in patterns)]


need("launch date April 14", with_all(r"launch", APRIL_14))
need("budget 42,000", with_all(r"budget|cost|funding|spend", B42))
need("Priya owns onboarding", with_all(r"priya", r"onboarding"))
need("Tomas owns the billing migration", with_all(r"tomas", r"billing"))
need("offsite in Lisbon", with_all(r"offsite|off-site", r"lisbon|lisboa"))
need("feature freeze March 31", with_all(r"freeze", MARCH_31))

# superseded values must not be presented as the decision
for s in segments:
    if re.search(MARCH_30, s) and not re.search(DROP, s):
        problems.append(f"the dropped launch idea is presented as a decision: {s!r}")
    if re.search(B38, s) and not re.search(DROP, s):
        problems.append(f"the superseded budget is presented as a decision: {s!r}")
    if re.search(r"priya", s) and re.search(r"billing", s) and not re.search(r"tomas", s):
        problems.append(f"wrong owner: {s!r}")
    if re.search(r"tomas", s) and re.search(r"onboarding", s) and not re.search(r"priya", s):
        problems.append(f"wrong owner: {s!r}")

# noise from the notes does not belong in a decisions summary
for noise in ("expense report", "status page", "welcome email", "retro", "attendees"):
    if noise in text:
        problems.append(f"noise from the notes: {noise!r}")

if problems:
    print("\n".join(problems))
    sys.exit(1)
print("ok")
PY
