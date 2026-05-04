#!/usr/bin/env bash
# Import the follow-up tasks from docs/research/followups-asana-import.csv into
# an Asana project via the REST API.
#
# Usage:
#   ASANA_ACCESS_TOKEN=<your_pat> \
#   ASANA_PROJECT_GID=<numeric_project_id> \
#   ASANA_WORKSPACE_GID=<numeric_workspace_id> \
#       scripts/asana_import_followups.sh [--dry-run]
#
# Get a personal access token: https://app.asana.com/0/my-apps
# Get the project GID by visiting the project URL — the long number in the URL.
# Get the workspace GID via:  curl -H "Authorization: Bearer $ASANA_ACCESS_TOKEN" \
#                                  https://app.asana.com/api/1.0/workspaces

set -euo pipefail

CSV_PATH="${CSV_PATH:-docs/research/followups-asana-import.csv}"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

if [[ -z "${ASANA_ACCESS_TOKEN:-}" ]]; then
    echo "error: set ASANA_ACCESS_TOKEN (https://app.asana.com/0/my-apps)" >&2
    exit 1
fi
if [[ -z "${ASANA_PROJECT_GID:-}" ]]; then
    echo "error: set ASANA_PROJECT_GID (numeric id from project URL)" >&2
    exit 1
fi
if [[ -z "${ASANA_WORKSPACE_GID:-}" ]]; then
    echo "error: set ASANA_WORKSPACE_GID (curl /workspaces with your token)" >&2
    exit 1
fi
if [[ ! -f "$CSV_PATH" ]]; then
    echo "error: CSV not found at $CSV_PATH" >&2
    exit 1
fi

# Cache section name -> gid lookups so we don't re-query per task.
declare -A SECTION_GIDS=()

create_section() {
    local name="$1"
    if [[ -n "${SECTION_GIDS[$name]:-}" ]]; then
        echo "${SECTION_GIDS[$name]}"
        return
    fi

    # Look up existing sections first — avoid duplicates if rerun.
    local existing
    existing=$(curl -fsSL -H "Authorization: Bearer $ASANA_ACCESS_TOKEN" \
        "https://app.asana.com/api/1.0/projects/$ASANA_PROJECT_GID/sections" \
        | jq -r --arg n "$name" '.data[] | select(.name == $n) | .gid' \
        | head -n1)
    if [[ -n "$existing" ]]; then
        SECTION_GIDS[$name]="$existing"
        echo "$existing"
        return
    fi

    if [[ $DRY_RUN -eq 1 ]]; then
        echo "DRY-SECTION-${name// /-}"
        return
    fi

    local gid
    gid=$(curl -fsSL -X POST \
        -H "Authorization: Bearer $ASANA_ACCESS_TOKEN" \
        -H "Content-Type: application/json" \
        -d "{\"data\":{\"name\":$(jq -Rs . <<<"$name")}}" \
        "https://app.asana.com/api/1.0/projects/$ASANA_PROJECT_GID/sections" \
        | jq -r '.data.gid')
    SECTION_GIDS[$name]="$gid"
    echo "$gid"
}

create_task() {
    local name="$1" notes="$2" section_gid="$3" tags_csv="$4" priority="$5"

    # Append priority to notes since custom fields require gids we don't know.
    local body_notes="$notes"
    if [[ -n "$priority" ]]; then
        body_notes="${body_notes}\n\nPriority: ${priority}"
    fi
    if [[ -n "$tags_csv" ]]; then
        body_notes="${body_notes}\nTags: ${tags_csv}"
    fi

    local payload
    payload=$(jq -n \
        --arg name "$name" \
        --arg notes "$body_notes" \
        --arg workspace "$ASANA_WORKSPACE_GID" \
        --arg project "$ASANA_PROJECT_GID" \
        --arg section "$section_gid" \
        '{data: {name: $name, notes: $notes, workspace: $workspace,
                  projects: [$project], memberships: [{project: $project, section: $section}]}}')

    if [[ $DRY_RUN -eq 1 ]]; then
        printf "  [DRY] %s\n" "$name"
        return
    fi

    local resp_gid
    resp_gid=$(curl -fsSL -X POST \
        -H "Authorization: Bearer $ASANA_ACCESS_TOKEN" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "https://app.asana.com/api/1.0/tasks" \
        | jq -r '.data.gid')
    printf "  [created %s] %s\n" "$resp_gid" "$name"
}

# Parse CSV with mlr (preferred — handles quoted commas safely) or fall back to
# awk. Brew install: `brew install miller`.
PARSER=""
if command -v mlr >/dev/null 2>&1; then
    PARSER="mlr"
elif python3 -c "import csv" >/dev/null 2>&1; then
    PARSER="python"
else
    echo "error: neither 'mlr' (Miller) nor python3 found; install one to parse the CSV safely." >&2
    exit 1
fi

echo "Importing tasks from $CSV_PATH into project $ASANA_PROJECT_GID"
[[ $DRY_RUN -eq 1 ]] && echo "(dry-run mode — no API writes)"

# Stream rows as TSV via mlr or python (so embedded commas in quoted fields are safe),
# then process each row in shell.
emit_rows() {
    if [[ "$PARSER" == "mlr" ]]; then
        mlr --c2t cat "$CSV_PATH"
    else
        python3 - "$CSV_PATH" <<'PY'
import csv, sys
with open(sys.argv[1], newline='') as f:
    r = csv.reader(f)
    header = next(r)
    print('\t'.join(header))
    for row in r:
        print('\t'.join(c.replace('\t', ' ').replace('\n', ' ') for c in row))
PY
    fi
}

current_section=""
current_section_gid=""

emit_rows | tail -n +2 | while IFS=$'\t' read -r name desc section tags priority; do
    [[ -z "$name" ]] && continue

    if [[ "$section" != "$current_section" ]]; then
        current_section="$section"
        echo "Section: $section"
        current_section_gid="$(create_section "$section")"
    fi

    create_task "$name" "$desc" "$current_section_gid" "$tags" "$priority"
done

echo
echo "Done. ${#SECTION_GIDS[@]} sections touched."
[[ $DRY_RUN -eq 0 ]] && echo "Review at https://app.asana.com/0/$ASANA_PROJECT_GID"
