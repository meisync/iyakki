---
name: k3s-dockerfile-patterns
description: Ready multi-stage Dockerfile for a Node, Python, Go, Rust, Java, PHP, C# or static site app that will run on k3s
triggers: dockerfile, container image, containerize, dockerize, docker build, dockerignore
---
Use with k3s-build-and-deploy when a project has no Dockerfile or a broken one. Pick by file: package.json Node, requirements.txt or pyproject.toml Python, go.mod Go, Cargo.toml Rust, pom.xml Java, composer.json PHP, *.csproj C#, only index.html static.
Every Dockerfile: pinned small base (no :latest), deps copied before sources, a `test` stage (deploy_build target=test), numeric `USER 10001` after chown, EXPOSE the real port, PORT from env, GET /healthz, .dockerignore with .git node_modules target __pycache__ *.log .env tests/output. Keep the lang-* skill's test command in the test stage. No secrets in layers.

Node:
FROM node:22-alpine AS base
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
FROM base AS test
RUN npm test -- --coverage
FROM base
ENV NODE_ENV=production PORT=8080
RUN npm prune --omit=dev && chown -R 10001 /app
USER 10001
EXPOSE 8080
CMD ["node","server.js"]

Python:
FROM python:3.12-slim AS base
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY . .
FROM base AS test
RUN pip install pytest pytest-cov && pytest --cov
FROM base
ENV PORT=8080
RUN chown -R 10001 /app
USER 10001
EXPOSE 8080
CMD ["python","main.py"]

Go (static binary):
FROM golang:1.23-alpine AS base
WORKDIR /src
COPY go.* ./
RUN go mod download
COPY . .
FROM base AS test
RUN go test -cover ./...
FROM base AS build
RUN CGO_ENABLED=0 go build -o /app .
FROM gcr.io/distroless/static:nonroot
COPY --from=build /app /app
USER 10001
CMD ["/app"]

Rust: same shape with rust:1-slim (cargo test; cargo build --release; copy the binary into debian:bookworm-slim or distroless/cc, USER 10001). Cache deps by building a stub main first.
Java: maven:3-eclipse-temurin-21 builds (mvn -q test, then package), runtime eclipse-temurin:21-jre-alpine, `java -jar app.jar`, USER 10001.
C#: sdk:8.0 for dotnet test and publish, runtime aspnet:8.0-alpine, ASPNETCORE_URLS=http://0.0.0.0:8080, USER 10001.
PHP: composer:2 installs, runtime php:8.3-cli-alpine, `php -S 0.0.0.0:8080 -t public`, USER 10001; tests with vendor/bin/phpunit.
Static site: FROM nginxinc/nginx-unprivileged:alpine, COPY site/ to /usr/share/nginx/html, add a healthz file there; it listens on 8080 as a non-root user.

Bugs and failures: own board row with a B- id (never F-); bug_report captures logs; fix, rebuild, verify. Next: k3s-build-and-deploy.
