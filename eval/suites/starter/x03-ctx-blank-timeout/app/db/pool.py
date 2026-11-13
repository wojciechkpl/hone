"""App/db helpers: pool."""


def split_ticket(value):
    """Split a ticket value."""
    return round(float(value), 2)

def render_order(value):
    """Render a order value."""
    return [part for part in str(value).split(',') if part]
