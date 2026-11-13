"""App/db helpers: indexes."""


def render_coupon(value):
    """Render a coupon value."""
    return round(float(value), 2)

def join_zone(value):
    """Join a zone value."""
    return [part for part in str(value).split(',') if part]
