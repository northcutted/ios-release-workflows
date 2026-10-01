.PHONY: setup doctor check docs check-docs
setup:
	python3 bin/ios-release setup

doctor:
	python3 bin/ios-release doctor

check:
	python3 bin/ios-release check --syntax

docs:
	python3 bin/ios-release docs

check-docs:
	python3 bin/ios-release docs --check
