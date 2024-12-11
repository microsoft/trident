# Harpoon & Nebraska

This document describes how to set up a Nebraska server for Harpoon.

## Dev Setup

(For more info, you can see the [upstream guide](https://www.flatcar.org/docs/latest/nebraska/authorization/).)

> ⚠️ **Warning** ⚠️
> 
> This setup is for development purposes only. Do not use it in production. It
> creates a very weakly protected and exposed database as well as a very exposed
> and auth-less Nebraska server.

### Prerequisites

- Docker
- PostgreSQL client

### Steps

1. Start a PostgreSQL server:

   ```bash
   # Create container
   docker run -d --name nebraska-postgres-dev -p 5432:5432 -e  POSTGRES_PASSWORD=nebraska postgres

   # Create database
   psql postgres://postgres:nebraska@localhost:5432/postgres -c 'create database  nebraska;'

   # Set timezone
   psql postgres://postgres:nebraska@localhost:5432/nebraska -c 'set timezone = "utc";'
   ```

2. Start a Nebraska server with a connection string to the posgresql server:

   ```bash
   docker run -d --name nebraska-test \
       --network=host \
       --env 'NEBRASKA_DB_URL=postgres://postgres:nebraska@localhost:5432/nebraska?sslmode=disable&connect_timeout=10' \
       ghcr.io/flatcar/nebraska:staging \
       /nebraska/nebraska \
       -http-static-dir=/nebraska/static \
       -auth-mode noop
   ```
