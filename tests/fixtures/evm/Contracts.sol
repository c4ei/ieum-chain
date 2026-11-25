// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC721/ERC721.sol";

contract Child { uint256 public born = block.number; }

contract TestToken is ERC20 {
    constructor(uint256 supply) ERC20("Test Meme", "MEME") { _mint(msg.sender, supply); }
    function boom() external pure { revert("boom"); }
    function spawn() external returns (address) { return address(new Child()); }
}

contract TestNft is ERC721 {
    constructor() ERC721("Test Badge", "BADGE") {}
    function mint(address to, uint256 id) external { _safeMint(to, id); }
}

contract Vault {
    event Deposited(address indexed from, uint256 amount);
    mapping(address => uint256) public deposits;
    receive() external payable { deposits[msg.sender] += msg.value; emit Deposited(msg.sender, msg.value); }
    function withdraw() external {
        uint256 amount = deposits[msg.sender];
        deposits[msg.sender] = 0;
        (bool ok, ) = msg.sender.call{value: amount}("");
        require(ok, "send failed");
    }
}
