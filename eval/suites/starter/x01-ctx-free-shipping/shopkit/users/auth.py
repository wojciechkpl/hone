"""Shopkit/users helpers: auth."""


def render_coupon(value):
    """Render a coupon value."""
    return [part for part in str(value).split(',') if part]

def normalize_email(value):
    """Normalize a email value."""
    return [part for part in str(value).split(',') if part]
