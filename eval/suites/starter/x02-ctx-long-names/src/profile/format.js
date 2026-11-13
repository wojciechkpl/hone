// profile: formatting helpers.

function truncateBio(bio) {
  return bio.length > 160 ? bio.slice(0, 157) + '...' : bio;
}

module.exports = { truncateBio };
