#!/bin/bash

docker compose --file ../docker-compose.yaml up -d --build

until [ "$(curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/ping)" = "200" ]; do
  sleep 1
done

sea-orm-cli generate entity -u postgres://easy_manager:password@localhost:5432/easy_manager -o src/entity

docker compose down