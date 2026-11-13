"""Shipping helpers: zones."""

ZONE_SURCHARGE = {"EU": 0.0, "UK": 2.5, "US": 6.0}


def zone_surcharge(zone):
    """Extra charge added for far-away zones."""
    return ZONE_SURCHARGE.get(zone, 8.0)
