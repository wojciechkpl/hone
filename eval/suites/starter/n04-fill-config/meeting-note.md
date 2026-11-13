# Service setup meeting

- The checkout service database moved. Host is db-staging.internal. The port was 5432 before
  but the new instance uses 5433.
- Retry attempts: we first said 3, then agreed on 4 attempts.
- Priya is the owner of the service.
- Feature flag new_checkout is switched on for staging; old_checkout stays off.
