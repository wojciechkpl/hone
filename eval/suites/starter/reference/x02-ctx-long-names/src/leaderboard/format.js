// leaderboard: formatting helpers.

const MAX_NAME_LENGTH = 12;

function formatPlayerName(name) {
  if (name.length <= MAX_NAME_LENGTH) {
    return name;
  }
  return name.slice(0, MAX_NAME_LENGTH - 1) + '…';
}

module.exports = { formatPlayerName };
