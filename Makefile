.PHONY: release-green
release-green:
	./run.sh release-green $(ARGS)

# `make release green` is the same gate: the word "green" is a goal that does nothing, so `release` runs it once.
.PHONY: release green
release:
	./run.sh release-green $(ARGS)
green:
	@:
