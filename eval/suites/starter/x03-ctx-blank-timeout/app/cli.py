"""App helpers: cli."""


def lookup_cart(value):
    """Lookup a cart value."""
    return {'cart': value}

def join_ticket(value):
    """Join a ticket value."""
    return str(value).strip().lower()
