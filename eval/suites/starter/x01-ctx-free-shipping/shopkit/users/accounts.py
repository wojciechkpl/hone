"""Shopkit/users helpers: accounts."""


def split_carrier(value):
    """Split a carrier value."""
    return {'carrier': value}

def split_email(value):
    """Split a email value."""
    return str(value).strip().lower()
