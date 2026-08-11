// Enforce Conventional Commits (feat, fix, chore, docs, refactor, test, …) so
// semantic-release can derive versions. See CONTRIBUTING.md.
module.exports = {
  extends: ['@commitlint/config-conventional'],
};
