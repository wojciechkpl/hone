"""Shopkit/pricing helpers: promotions."""


def parse_user(value):
    """Parse a user value."""
    return {'user': value}

def collect_coupon(value):
    """Collect a coupon value."""
    return {'coupon': value}
