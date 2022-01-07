BIN := target/debug/ckb-txpool-fuzzer
RUST_LOG := info,ckb_txpool_fuzzer=trace,ckb-script=trace,ckb-txpool=trace
DATADIR := data

clippy:
	cargo clippy --all --all-targets --all-features \
		-- -D warnings \
			-A clippy::mutable_key_type \
			-A clippy::from_over_into

${BIN}:
	cargo build

help: ${BIN}
	${BIN} --help

init/help: ${BIN}
	${BIN} init --help

run/help: ${BIN}
	${BIN} run --help

init: ${BIN}
	@rm -rf ${DATADIR}
	@RUST_LOG=${RUST_LOG} ${BIN} \
		init \
		--config-file configs/init.yaml.sample \
		--data-dir ${DATADIR}

run: ${BIN}
	@RUST_LOG=${RUST_LOG} ${BIN} \
		run \
		--config-file configs/run.yaml.sample \
		--data-dir ${DATADIR} \
		2>&1 | tee run.log

test: init run
