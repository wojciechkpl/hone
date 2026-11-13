"""A correct refactor that spells the rule differently."""

RATE = 0.15


def apply_discount(price):
    return round(price * (1 - RATE), 2)


def member_price(price):
    return apply_discount(price)


def bundle_price(prices):
    return round(sum(apply_discount(p) for p in prices), 2)


def clearance_price(price, extra_off=0):
    return round(apply_discount(price) - extra_off, 2)
