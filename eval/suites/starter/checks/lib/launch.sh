# Sourced by the judge-based checks (T19): launch_judge <task-id>
# Copies the judge and everything it needs into a private, read-only bundle directory and execs
# it from there with EVAL_SUITE_DIR removed (`exec env -u`), so no process an agent child can
# inspect has the suite path in its argv or environment. The judge loads the bundle into memory
# and deletes it before it starts any solution process (T23).
#
# T24: first, bundles an earlier judge left behind (it was killed before it could delete its
# bundle) are swept: only `evalchk.*` / `evalchk-*` entries of $TMPDIR older than an hour.
launch_judge() {
  _id=$1
  _tmp="${TMPDIR:-/tmp}"
  python3 -I -B "$EVAL_SUITE_DIR/checks/lib/sweep.py" "$_tmp"
  _b=$(mktemp -d "$_tmp/evalchk.XXXXXX") || exit 1
  python3 -I -B "$EVAL_SUITE_DIR/checks/lib/bundle.py" "$EVAL_SUITE_DIR" "$_b" "$_id" || exit 1
  chmod -R a-w "$_b"
  exec env -u EVAL_SUITE_DIR python3 -I -B "$_b/checks/lib/judge.py" "$_id"
}
