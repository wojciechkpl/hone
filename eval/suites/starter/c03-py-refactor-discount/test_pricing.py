import unittest

import pricing


class PricingTest(unittest.TestCase):
    def test_member_price(self):
        self.assertEqual(pricing.member_price(100), 85.0)

    def test_bundle_price(self):
        self.assertEqual(pricing.bundle_price([100, 20]), 102.0)

    def test_clearance_price(self):
        self.assertEqual(pricing.clearance_price(100, extra_off=5), 80.0)


if __name__ == "__main__":
    unittest.main()
