# test stage: the build fails if the unit tests fail
FROM python:3.12.7-slim AS test
WORKDIR /app
COPY server.py ./
COPY tests ./tests
RUN python3 tests/test_server.py

FROM python:3.12.7-slim
RUN useradd --system --uid 10001 app
WORKDIR /app
COPY server.py ./
USER 10001
ENV PORT=8080
EXPOSE 8080
CMD ["python3", "server.py"]
