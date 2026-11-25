# EVM 테스트 계약

`Contracts.sol`을 solc 0.8.30(evmVersion `prague`, optimizer 200)과 OpenZeppelin Contracts 5로
컴파일한 배포 바이트코드입니다. `src/evm.rs` 테스트가 사용합니다.

다시 만들기:

```bash
npm i solc@0.8.30 @openzeppelin/contracts@5
# compile.js: standard JSON 입력으로 TestToken, TestNft, Vault의 evm.bytecode.object를 *.hex로 저장
```
