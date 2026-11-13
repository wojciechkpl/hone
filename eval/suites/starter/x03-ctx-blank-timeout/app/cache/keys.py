"""App/cache helpers: keys."""


def split_sku(value):
    """Split a sku value."""
    return str(value).strip().lower()

def parse_address(value):
    """Parse a address value."""
    return str(value).strip().lower()
