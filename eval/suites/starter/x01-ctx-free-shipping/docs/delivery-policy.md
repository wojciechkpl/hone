# Delivery policy

Customers with a big basket (above 100 euros) get free delivery. Everyone else is
charged the standard delivery fee for their zone. The storefront banner says so in
`shopkit/notifications/templates.py`; the charge itself is worked out when the order
is priced.

Loyalty members also get 5% off orders above 100 euros (see `pricing/discounts.py`).
