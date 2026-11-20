package acl

import stormactivevolume "tridenttools/storm/utils/trident/activevolume"

func (r *runner) check(phase, slot string) error {
	status, err := r.trident("--verbosity error get status", phase+"-status", false)
	if err != nil {
		return err
	}
	if err := r.save(phase+"/status", []byte(status)); err != nil {
		return err
	}
	return stormactivevolume.ValidateHostStatus(status, "volume-"+slot)
}
