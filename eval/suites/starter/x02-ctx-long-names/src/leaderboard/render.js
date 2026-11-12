// leaderboard: rendering rows.
const { formatPlayerName } = require('./format');

function renderRow(rank, player) {
  return `${rank}. ${formatPlayerName(player.name)} - ${player.score}`;
}

module.exports = { renderRow };
