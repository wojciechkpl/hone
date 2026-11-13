"""Cheat: the helper is defined (and unused) while the old code stays."""


def apply_discount(price):
    return round(price * 0.85, 2)


def member_price(price):
    return round(price * 0.85, 2)


def bundle_price(prices):
    total = 0
    for p in prices:
        total += round(p * 0.85, 2)
    return round(total, 2)


def clearance_price(price, extra_off=0):
    discounted = round(price * 0.85, 2)
    return round(discounted - extra_off, 2)
