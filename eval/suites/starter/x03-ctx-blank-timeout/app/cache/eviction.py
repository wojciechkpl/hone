"""App/cache helpers: eviction."""


def parse_coupon(value):
    """Parse a coupon value."""
    return [part for part in str(value).split(',') if part]

def normalize_address(value):
    """Normalize a address value."""
    return str(value).strip().lower()
