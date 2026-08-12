#!/usr/bin/env bash
# Load the fixture schema into the test databases.
#
#   docker compose -f tests/docker-compose.test.yml up -d
#   ./scripts/seed.sh
#
# The SQLite fixture needs no container and is always built. Every container
# engine is seeded if it is running, and skipped with a hint if it is not, so
# this is safe to run whatever subset you have up.
set -euo pipefail

cd "$(dirname "$0")/.."
COMPOSE="docker compose -f tests/docker-compose.test.yml"

# Matches tests/docker-compose.test.yml. SQL Server enforces a complexity rule, so it
# cannot share the others' password.
MSSQL_PASSWORD='Faro!Passw0rd'

running() {
  $COMPOSE ps --services 2>/dev/null | grep -qx "$1"
}

skip() {
  echo "==> $1 container not running; skipped."
  echo "    Start it with: docker compose -f tests/docker-compose.test.yml up -d $2"
}

mkdir -p tests/fixtures

echo "==> SQLite"
rm -f tests/fixtures/faro_test.db tests/fixtures/faro_test.db-wal tests/fixtures/faro_test.db-shm
sqlite3 tests/fixtures/faro_test.db < scripts/seed/sqlite.sql
echo "    tests/fixtures/faro_test.db"

if running postgres; then
  echo "==> PostgreSQL"
  # Wait for readiness: the container accepts connections before the server does.
  IS_READY=0
  for _ in $(seq 1 60); do
    if $COMPOSE exec -T postgres pg_isready -U faro -d faro_test >/dev/null 2>&1; then
      IS_READY=1
      break
    fi
    sleep 1
  done
  if [ "$IS_READY" -ne 1 ]; then
    echo "PostgreSQL container failed to become ready in 60s" >&2
    exit 1
  fi
  $COMPOSE exec -T postgres \
    psql -U faro -d faro_test -v ON_ERROR_STOP=1 < scripts/seed/postgres.sql >/dev/null
  echo "    localhost:55432/faro_test (user faro, password faro)"
else
  skip PostgreSQL postgres
fi

if running mysql; then
  echo "==> MySQL"
  IS_READY=0
  for _ in $(seq 1 60); do
    if $COMPOSE exec -T mysql mysqladmin ping -h 127.0.0.1 -ufaro -pfaro >/dev/null 2>&1; then
      IS_READY=1
      break
    fi
    sleep 1
  done
  if [ "$IS_READY" -ne 1 ]; then
    echo "MySQL container failed to become ready in 60s" >&2
    exit 1
  fi
  $COMPOSE exec -T mysql mysql --default-character-set=utf8mb4 \
    -ufaro -pfaro faro_test < scripts/seed/mysql.sql
  echo "    localhost:53306/faro_test (user faro, password faro)"
else
  skip MySQL mysql
fi

if running mariadb; then
  echo "==> MariaDB"
  IS_READY=0
  for _ in $(seq 1 60); do
    if $COMPOSE exec -T mariadb mariadb-admin ping -h 127.0.0.1 -ufaro -pfaro >/dev/null 2>&1; then
      IS_READY=1
      break
    fi
    sleep 1
  done
  if [ "$IS_READY" -ne 1 ]; then
    echo "MariaDB container failed to become ready in 60s" >&2
    exit 1
  fi
  # MariaDB shares the MySQL fixture; its dialect differences do not reach it.
  $COMPOSE exec -T mariadb mariadb --default-character-set=utf8mb4 \
    -ufaro -pfaro faro_test < scripts/seed/mysql.sql
  echo "    localhost:53307/faro_test (user faro, password faro)"
else
  skip MariaDB mariadb
fi

if running sqlserver; then
  echo "==> SQL Server"
  SQLCMD="/opt/mssql-tools18/bin/sqlcmd"
  IS_READY=0
  for _ in $(seq 1 120); do
    if $COMPOSE exec -T sqlserver "$SQLCMD" \
        -S localhost -U sa -P "$MSSQL_PASSWORD" -C -Q "SELECT 1" >/dev/null 2>&1; then
      IS_READY=1
      break
    fi
    sleep 2
  done
  if [ "$IS_READY" -ne 1 ]; then
    echo "SQL Server container failed to become ready in 240s" >&2
    exit 1
  fi
  # -C trusts the container's self-signed certificate; -b fails the script on
  # the first error rather than reporting success over a broken fixture.
  $COMPOSE exec -T sqlserver "$SQLCMD" \
    -S localhost -U sa -P "$MSSQL_PASSWORD" -C -b \
    -i /dev/stdin < scripts/seed/sqlserver.sql >/dev/null
  echo "    localhost:51433/faro_test (user sa, password $MSSQL_PASSWORD)"
else
  skip "SQL Server" sqlserver
fi

if running clickhouse; then
  echo "==> ClickHouse"
  IS_READY=0
  for _ in $(seq 1 60); do
    if curl -fsS -m 3 -u faro:faro \
        "http://127.0.0.1:58123/?query=SELECT%201" >/dev/null 2>&1; then
      IS_READY=1
      break
    fi
    sleep 1
  done
  if [ "$IS_READY" -ne 1 ]; then
    echo "ClickHouse container failed to become ready in 60s" >&2
    exit 1
  fi
  # Statements are sent one at a time: ClickHouse's HTTP interface takes a single
  # statement per request, so a whole script cannot simply be piped in.
  python3 - <<'PY'
import re, sys, urllib.error, urllib.request, base64

sql = open("scripts/seed/clickhouse.sql").read()
# Strip comments, then split on semicolons at the end of a line so the
# multi-row VALUES lists intact.
sql = re.sub(r"^\s*--.*$", "", sql, flags=re.M)
statements = [s.strip() for s in sql.split(";\n") if s.strip()]

auth = base64.b64encode(b"faro:faro").decode()
for statement in statements:
    request = urllib.request.Request(
        "http://127.0.0.1:58123/?database=faro_test",
        data=statement.encode(),
        headers={"Authorization": f"Basic {auth}"},
    )
    try:
        urllib.request.urlopen(request).read()
    except urllib.error.HTTPError as e:
        sys.exit(f"ClickHouse rejected a statement:\n{statement[:200]}\n\n{e.read().decode()}")
PY
  echo "    localhost:58123/faro_test (user faro, password faro)"
else
  skip ClickHouse clickhouse
fi

if running mongo; then
  echo "==> MongoDB"
  IS_READY=0
  for _ in $(seq 1 60); do
    if $COMPOSE exec -T mongo mongosh --quiet -u faro -p faro \
        --authenticationDatabase admin --eval 'db.runCommand({ping:1})' >/dev/null 2>&1; then
      IS_READY=1
      break
    fi
    sleep 1
  done
  if [ "$IS_READY" -ne 1 ]; then
    echo "MongoDB container failed to become ready in 60s" >&2
    exit 1
  fi
  $COMPOSE exec -T mongo mongosh --quiet -u faro -p faro \
    --authenticationDatabase admin --file /dev/stdin < scripts/seed/mongo.js >/dev/null
  echo "    localhost:57017/faro_test (user faro, password faro)"
else
  skip MongoDB mongo
fi

echo
echo "Done."
