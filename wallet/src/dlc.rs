use crate::ErnestWallet;
use dlc::PartyParams;

impl ErnestWallet {
    pub fn create_party_params(
        &self,
        input_amount: u64,
        collateral: u64,
    ) -> anyhow::Result<PartyParams> {
        let fund_pubkey = self.get_pubkey()?;

        let change_script_pubkey = self.new_change_address()?;
        let payout_script_pubkey = self.new_external_address()?;

        // Inputs? Need to select coins that equal the input amount/collateral

        let party_params = PartyParams {
            fund_pubkey,
            change_script_pubkey: change_script_pubkey.script_pubkey(),
            payout_script_pubkey: payout_script_pubkey.script_pubkey(),
            change_serial_id: 0,
            payout_serial_id: 0,
            inputs: Vec::new(),
            input_amount,
            collateral,
        };
        Ok(party_params)
    }
}
