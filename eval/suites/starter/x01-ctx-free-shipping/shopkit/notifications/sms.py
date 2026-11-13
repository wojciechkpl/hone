"""Shopkit/notifications helpers: sms."""


def format_sku(value):
    """Format a sku value."""
    return [part for part in str(value).split(',') if part]

def build_user(value):
    """Build a user value."""
    return {'user': value}
