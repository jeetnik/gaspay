const anchor = require('@coral-xyz/anchor');
const { PublicKey, SystemProgram } = require('@solana/web3.js');

// Program ID
const PROGRAM_ID = new PublicKey('368SCgsps98BfdQfgcZvmhexXXijABFWZVj5PDjUWtyi');

async function initializeProgram() {
    // Set up provider
    const provider = anchor.AnchorProvider.env();
    anchor.setProvider(provider);
    
    // Load the program
    const program = anchor.workspace.FeePaymentDapp;
    
    // Admin wallet (your wallet)
    const admin = provider.wallet.publicKey;
    
    // Derive state PDA
    const [statePda] = PublicKey.findProgramAddressSync(
        [Buffer.from('state')],
        PROGRAM_ID
    );
    
    try {
        // Initialize the program
        const tx = await program.methods
            .initialize(admin)
            .accounts({
                state: statePda,
                admin: admin,
                systemProgram: SystemProgram.programId,
            })
            .rpc();
        
        console.log('Program initialized successfully!');
        console.log('Transaction signature:', tx);
        console.log('State PDA:', statePda.toString());
    } catch (error) {
        console.error('Initialization failed:', error);
    }
}

initializeProgram();