#!/bin/bash
# Table-driven test for count_blocking_findings in pre-push-review.sh.
# The counter is the last line of defense before a push: it must skip
# empty-section markers but NEVER undercount real findings. Run directly:
#
#   .claude/hooks/test-pre-push-counter.sh
#
# Exits 0 if all cases pass, 1 otherwise.

set -uo pipefail

HOOK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Extract the function under test from the hook so the test always runs
# against the real implementation, not a copy.
eval "$(sed -n '/^count_blocking_findings()/,/^}$/p' "$HOOK_DIR/pre-push-review.sh")"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

FAILED=0
run_case() {
  local name="$1" expected="$2" input="$3"
  local file="$TMPDIR/case.txt"
  printf '%s\n' "$input" > "$file"
  local got
  got=$(count_blocking_findings "$file")
  if [ "$got" = "$expected" ]; then
    echo "PASS: $name (count=$got)"
  else
    echo "FAIL: $name (expected $expected, got $got)"
    FAILED=1
  fi
}

run_case "unbulleted None. is ignored" 0 '## Review
### Critical Issues
None.
### Important Issues
None.'

run_case "bulleted empty markers are skipped" 0 '## Review
### Critical Issues
- None.
### Important Issues
- none
### Warnings
- **None found.**'

run_case "no-issues variants are skipped" 0 '## Review
### Critical Issues
- No issues found.
### Warnings
- No concerns identified.'

run_case "real findings count" 2 '## Review
### Critical Issues
- [file.rs:1] Buffer overflow in parser
### Important Issues
- [file.rs:2] Missing error handling'

run_case "findings starting with None still count" 1 '## Review
### Critical Issues
- None of the inputs are validated'

run_case "None. with trailing prose still counts (fail closed)" 1 '## Review
### Important Issues
- None. Actually the auth check is missing here'

run_case "no-noun with trailing prose still counts (fail closed)" 1 '## Review
### Warnings
- No issues found in the auth module, but the deposit path overflows'

run_case "non-blocking sections are never counted" 0 '## Review
### Critical Issues
None.
### Nits
- style nit
### Notes
- observation'

run_case "section ends at next header" 1 '## Review
### Warnings
- real warning
### Summary
- summary bullet is not a finding'

exit $FAILED
