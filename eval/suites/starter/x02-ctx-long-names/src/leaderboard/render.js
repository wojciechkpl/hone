// leaderboard: rendering rows (the handle column is 12 characters wide).
const { formatPlayerName } = require('./format');

function renderRow(rank, player) {
  return `${rank}. ${formatPlayerName(player.name)} - ${player.score}`;
}

module.exports = { renderRow };
