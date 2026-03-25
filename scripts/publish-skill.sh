#!/usr/bin/env bash
# publish-skill.sh — Validate and verify the tauri-connector skill for skills.sh
#
# skills.sh does not have a traditional "publish" step. Skills are GitHub repos
# that users install with: npx skills add <owner>/<repo>
#
# The leaderboard at skills.sh is populated automatically via anonymous telemetry
# when users run `npx skills add`. No registration or API key is needed.
#
# This script validates the skill structure, tests that the skills CLI can
# discover the skill from this repo, and provides the install commands.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SKILL_DIR="${REPO_ROOT}/skill"
SKILL_MD="${SKILL_DIR}/SKILL.md"
SETUP_MD="${SKILL_DIR}/SETUP.md"
SCRIPTS_DIR="${SKILL_DIR}/scripts"
REFS_DIR="${SKILL_DIR}/references"

# GitHub owner/repo from git remote
REMOTE_URL="$(git -C "${REPO_ROOT}" remote get-url origin 2>/dev/null || true)"
GITHUB_SLUG=""
if [[ "${REMOTE_URL}" =~ github\.com[:/]([^/]+/[^/.]+) ]]; then
  GITHUB_SLUG="${BASH_REMATCH[1]}"
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

errors=0
warnings=0

log_ok()   { printf "${GREEN}  [OK]${NC} %s\n" "$1"; }
log_warn() { printf "${YELLOW}[WARN]${NC} %s\n" "$1"; warnings=$((warnings + 1)); }
log_fail() { printf "${RED}[FAIL]${NC} %s\n" "$1"; errors=$((errors + 1)); }
log_info() { printf "${CYAN}[INFO]${NC} %s\n" "$1"; }

printf "\n${BOLD}=== tauri-connector skill validation ===${NC}\n\n"

# --- 1. Check required files ---
printf "${BOLD}1. Required files${NC}\n"

if [[ -f "${SKILL_MD}" ]]; then
  log_ok "skill/SKILL.md exists"
else
  log_fail "skill/SKILL.md not found"
fi

if [[ -f "${SETUP_MD}" ]]; then
  log_ok "skill/SETUP.md exists"
else
  log_warn "skill/SETUP.md not found (optional but recommended)"
fi

if [[ -d "${SCRIPTS_DIR}" ]]; then
  script_count=$(find "${SCRIPTS_DIR}" -name '*.ts' -type f | wc -l | tr -d ' ')
  log_ok "skill/scripts/ exists (${script_count} TypeScript scripts)"
else
  log_warn "skill/scripts/ not found (bun fallback scripts)"
fi

if [[ -d "${REFS_DIR}" ]]; then
  ref_count=$(find "${REFS_DIR}" -name '*.md' -type f | wc -l | tr -d ' ')
  log_ok "skill/references/ exists (${ref_count} reference files)"
else
  log_warn "skill/references/ not found (progressive disclosure references)"
fi

# --- 2. Validate SKILL.md frontmatter ---
printf "\n${BOLD}2. SKILL.md frontmatter${NC}\n"

if [[ -f "${SKILL_MD}" ]]; then
  # Extract YAML frontmatter between --- delimiters
  frontmatter=$(sed -n '/^---$/,/^---$/p' "${SKILL_MD}" | sed '1d;$d')

  # Check required fields
  if echo "${frontmatter}" | grep -q '^name:'; then
    name=$(echo "${frontmatter}" | grep '^name:' | sed 's/^name:[[:space:]]*//' | tr -d '"')
    log_ok "name: ${name}"
  else
    log_fail "Missing required field: name"
  fi

  if echo "${frontmatter}" | grep -q '^description:'; then
    desc_len=$(echo "${frontmatter}" | sed -n '/^description:/,/^[a-z]/p' | head -1 | wc -c | tr -d ' ')
    if [[ ${desc_len} -gt 50 ]]; then
      log_ok "description present (${desc_len} chars)"
    else
      log_warn "description is short (${desc_len} chars) -- longer descriptions help discoverability"
    fi
  else
    log_fail "Missing required field: description"
  fi
fi

# --- 3. Validate skill content ---
printf "\n${BOLD}3. Skill content checks${NC}\n"

if [[ -f "${SKILL_MD}" ]]; then
  line_count=$(wc -l < "${SKILL_MD}" | tr -d ' ')
  log_info "SKILL.md: ${line_count} lines"

  if [[ ${line_count} -lt 50 ]]; then
    log_warn "SKILL.md is quite short (${line_count} lines) -- skills with more content are more useful"
  elif [[ ${line_count} -gt 1000 ]]; then
    log_warn "SKILL.md is very long (${line_count} lines) -- consider using references/ for progressive disclosure"
  else
    log_ok "SKILL.md length is good (${line_count} lines)"
  fi

  # Check for code blocks (skills with examples are more useful)
  code_block_count=$(grep -c '```' "${SKILL_MD}" 2>/dev/null || echo "0")
  if [[ ${code_block_count} -gt 0 ]]; then
    log_ok "Contains code examples (${code_block_count} code fences)"
  else
    log_warn "No code blocks found -- examples help agents use the skill"
  fi

  # Check for section headings
  heading_count=$(grep -c '^##' "${SKILL_MD}" 2>/dev/null || echo "0")
  if [[ ${heading_count} -gt 2 ]]; then
    log_ok "Well-structured (${heading_count} sections)"
  else
    log_warn "Few sections (${heading_count}) -- more structure helps agents navigate"
  fi
fi

# --- 4. Check scripts are valid ---
printf "\n${BOLD}4. Bun scripts validation${NC}\n"

if [[ -d "${SCRIPTS_DIR}" ]]; then
  invalid_scripts=0
  for script in "${SCRIPTS_DIR}"/*.ts; do
    if [[ -f "${script}" ]]; then
      basename_script=$(basename "${script}")
      # Check that each script has a shebang or import (basic syntax check)
      first_line=$(head -1 "${script}")
      if [[ "${first_line}" == *"import"* ]] || [[ "${first_line}" == *"//"* ]] || [[ "${first_line}" == "#!/"* ]] || [[ "${first_line}" == "/**" ]] || [[ "${first_line}" == *"export"* ]] || [[ "${first_line}" == *"const"* ]]; then
        : # valid
      else
        log_warn "${basename_script}: first line doesn't look like TypeScript"
        invalid_scripts=$((invalid_scripts + 1))
      fi
    fi
  done
  if [[ ${invalid_scripts} -eq 0 ]]; then
    log_ok "All scripts have valid structure"
  fi
else
  log_info "No scripts directory to validate"
fi

# --- 5. Git state ---
printf "\n${BOLD}5. Git state${NC}\n"

if git -C "${REPO_ROOT}" diff --quiet -- skill/ 2>/dev/null; then
  log_ok "skill/ directory has no uncommitted changes"
else
  log_warn "skill/ directory has uncommitted changes -- commit before publishing"
fi

if [[ -n "${GITHUB_SLUG}" ]]; then
  log_ok "GitHub remote: ${GITHUB_SLUG}"
else
  log_warn "Could not determine GitHub owner/repo from git remote"
fi

# Check that skill/ is not gitignored
if git -C "${REPO_ROOT}" check-ignore -q skill/SKILL.md 2>/dev/null; then
  log_fail "skill/SKILL.md is gitignored -- skills.sh needs it in the repo"
else
  log_ok "skill/SKILL.md is tracked by git"
fi

# --- 6. Test skills CLI discovery ---
printf "\n${BOLD}6. Skills CLI discovery test${NC}\n"

if command -v npx &>/dev/null; then
  log_info "Testing skill discovery with: npx skills add ${GITHUB_SLUG:-<owner>/<repo>} --list"
  log_info "(This clones the repo and scans for SKILL.md files)"

  if [[ -n "${GITHUB_SLUG}" ]]; then
    # Run the list command to verify the skill is discoverable
    list_output=$(npx skills add "${GITHUB_SLUG}" --list 2>&1) || true
    if echo "${list_output}" | grep -qi "tauri-connector"; then
      log_ok "Skills CLI discovered the tauri-connector skill"
    else
      log_warn "Skills CLI did not find the skill -- this may be a cache issue"
      log_info "Output: ${list_output}"
    fi
  else
    log_info "Skipping remote discovery test (no GitHub slug detected)"
  fi

  # Test local discovery
  log_info "Testing local discovery..."
  list_local=$(npx skills add "${REPO_ROOT}" --list 2>&1) || true
  if echo "${list_local}" | grep -qi "tauri-connector"; then
    log_ok "Local discovery found the tauri-connector skill"
  else
    log_warn "Local discovery did not find the skill"
    log_info "Output: ${list_local}"
  fi
else
  log_warn "npx not found -- cannot test skills CLI discovery"
fi

# --- Summary ---
printf "\n${BOLD}=== Summary ===${NC}\n\n"

if [[ ${errors} -gt 0 ]]; then
  printf "${RED}${errors} error(s)${NC}, ${YELLOW}${warnings} warning(s)${NC}\n"
  printf "\nFix the errors above before publishing.\n"
  exit 1
elif [[ ${warnings} -gt 0 ]]; then
  printf "${GREEN}0 errors${NC}, ${YELLOW}${warnings} warning(s)${NC}\n"
else
  printf "${GREEN}All checks passed.${NC}\n"
fi

printf "\n${BOLD}=== Install commands ===${NC}\n\n"

if [[ -n "${GITHUB_SLUG}" ]]; then
  printf "Users can install this skill with:\n\n"
  printf "  ${CYAN}npx skills add ${GITHUB_SLUG}${NC}\n\n"
  printf "Or install to a specific agent:\n\n"
  printf "  ${CYAN}npx skills add ${GITHUB_SLUG} -a claude-code${NC}\n"
  printf "  ${CYAN}npx skills add ${GITHUB_SLUG} -a cursor${NC}\n"
  printf "  ${CYAN}npx skills add ${GITHUB_SLUG} -a claude-code -g${NC}  (global)\n\n"
  printf "Or install from a direct URL:\n\n"
  printf "  ${CYAN}npx skills add https://github.com/${GITHUB_SLUG}/tree/main/skill${NC}\n\n"
  printf "List available skills:\n\n"
  printf "  ${CYAN}npx skills add ${GITHUB_SLUG} --list${NC}\n\n"
  printf "Leaderboard: ${CYAN}https://skills.sh/${GITHUB_SLUG}${NC}\n"
  printf "Skill page (once installed by users): ${CYAN}https://skills.sh/s/tauri-connector${NC}\n"
else
  printf "Could not determine install command (no GitHub remote).\n"
  printf "Push to GitHub and run again.\n"
fi

printf "\n"
exit 0
