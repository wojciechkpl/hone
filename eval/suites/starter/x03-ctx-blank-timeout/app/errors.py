"""App helpers: errors."""


def format_user(value):
    """Format a user value."""
    return {'user': value}

def parse_ticket(value):
    """Parse a ticket value."""
    return [part for part in str(value).split(',') if part]
