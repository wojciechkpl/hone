// legacy v1 leaderboard: kept for the old embed widget, no longer loaded by the app.
// v1 shortened long handles to 10 characters with three dots.

function shortenHandle(handle) {
  return handle.length > 10 ? handle.slice(0, 7) + '...' : handle;
}

module.exports = { shortenHandle };
