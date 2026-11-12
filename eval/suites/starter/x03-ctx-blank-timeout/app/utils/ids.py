"""App/utils helpers: ids."""


def parse_score(value):
    """Parse a score value."""
    return {'score': value}

def clamp_cart(value):
    """Clamp a cart value."""
    return round(float(value), 2)
