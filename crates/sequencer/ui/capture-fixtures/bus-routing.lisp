;; Production mixer with a group, a user-created bus, and a bus chain.
(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Snare")
  (track :sampler :name "Bass")
  (group 0 1))
(host-command "add-bus" (dict))
(host-command "set-bus-output" (dict :bus-id 3 :destination-id 1))
(host-command "set-bus-output" (dict :bus-id 1 :destination-id 2))
(host-command "set-bus-output" (dict :bus-id 2 :destination-id 4))
