#!/usr/bin/env bash
# Applies CodingPad's GitHub repository protections (D63). Safe to re-run: it updates what exists.
# Needs `gh` signed in as a repository admin, and a public repository (rulesets are not free on
# private ones). Usage: tools/github/configure-repo.sh [owner/repo]
set -euo pipefail

repo=${1:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}
actions_app=15368 # GitHub Actions; required checks must come from it, not from a commit status.

if [ "$(gh api "repos/$repo" -q .private)" = "true" ]; then
  echo "$repo is private. Make it public first (Settings > General > Danger Zone)." >&2
  exit 1
fi

echo "Merge settings: squash only, delete merged branches"
gh api -X PATCH "repos/$repo" --silent --input - <<'JSON'
{
  "allow_squash_merge": true,
  "allow_merge_commit": false,
  "allow_rebase_merge": false,
  "allow_auto_merge": true,
  "delete_branch_on_merge": true,
  "squash_merge_commit_title": "PR_TITLE",
  "squash_merge_commit_message": "PR_BODY",
  "security_and_analysis": {
    "secret_scanning": { "status": "enabled" },
    "secret_scanning_push_protection": { "status": "enabled" }
  }
}
JSON

echo "Dependabot alerts"
gh api -X PUT "repos/$repo/vulnerability-alerts" --silent

echo "Actions: read-only token, no PR approvals by workflows, approve every outside contributor's runs"
gh api -X PUT "repos/$repo/actions/permissions/workflow" --silent \
  -f default_workflow_permissions=read -F can_approve_pull_request_reviews=false
gh api -X PUT "repos/$repo/actions/permissions/fork-pr-contributor-approval" --silent \
  -f approval_policy=all_external_contributors

echo "CodeQL default setup"
gh api -X PATCH "repos/$repo/code-scanning/default-setup" --silent -f state=configured

upsert_ruleset() {
  local name=$1 body=$2 id
  id=$(gh api "repos/$repo/rulesets" --paginate -q ".[] | select(.name == \"$name\") | .id")
  if [ -n "$id" ]; then
    echo "Ruleset '$name': update"
    gh api -X PUT "repos/$repo/rulesets/$id" --silent --input - <<<"$body"
  else
    echo "Ruleset '$name': create"
    gh api -X POST "repos/$repo/rulesets" --silent --input - <<<"$body"
  fi
}

# master: PRs only (zero approvals), CI green and up to date, linear history, no force push or delete.
# Nobody bypasses it, including admins.
upsert_ruleset "master" "$(cat <<JSON
{
  "name": "master",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "bypass_actors": [],
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    { "type": "required_linear_history" },
    { "type": "pull_request", "parameters": {
        "required_approving_review_count": 0,
        "dismiss_stale_reviews_on_push": false,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": false,
        "allowed_merge_methods": ["squash"] } },
    { "type": "required_status_checks", "parameters": {
        "strict_required_status_checks_policy": true,
        "do_not_enforce_on_create": false,
        "required_status_checks": [
          { "context": "Linux", "integration_id": $actions_app },
          { "context": "Windows", "integration_id": $actions_app } ] } }
  ]
}
JSON
)"

# Release tags: only repository admins may create, move or delete a v* tag, since a tag publishes a release.
upsert_ruleset "release tags" "$(cat <<'JSON'
{
  "name": "release tags",
  "target": "tag",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
  "bypass_actors": [ { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always" } ],
  "rules": [
    { "type": "creation" },
    { "type": "update" },
    { "type": "deletion" },
    { "type": "non_fast_forward" }
  ]
}
JSON
)"

cat <<EOF

Done. One setting has no API yet; set it by hand:
  https://github.com/$repo/settings > General > Features > Pull requests > "Collaborators only"
EOF
