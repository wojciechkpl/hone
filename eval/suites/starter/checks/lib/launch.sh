# Sourced by the judge-based checks (T19): launch_judge <task-id>
# Copies the judge and everything it needs into a private, read-only bundle directory and execs
# it from there with EVAL_SUITE_DIR removed (`exec env -u`), so no process an agent child can
# inspect has the suite path in its argv or environment. The bundle is removed by the judge.
launch_judge() {
  _id=$1
  _b=$(mktemp -d "${TMPDIR:-/tmp}/evalchk.XXXXXX") || exit 1
  python3 -I -B "$EVAL_SUITE_DIR/checks/lib/bundle.py" "$EVAL_SUITE_DIR" "$_b" "$_id" || exit 1
  chmod -R a-w "$_b"
  exec env -u EVAL_SUITE_DIR python3 -I -B "$_b/checks/lib/judge.py" "$_id"
}
