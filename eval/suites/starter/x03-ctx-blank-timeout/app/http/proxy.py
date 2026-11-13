"""App/http helpers: proxy."""


def lookup_coupon(value):
    """Lookup a coupon value."""
    return str(value).strip().lower()

def collect_address(value):
    """Collect a address value."""
    return [part for part in str(value).split(',') if part]
