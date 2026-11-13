// feed: post truncation.

function truncatePost(text) {
  return text.length > 140 ? text.slice(0, 139) + '…' : text;
}

module.exports = { truncatePost };
